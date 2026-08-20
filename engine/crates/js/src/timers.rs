//! `setTimeout`/`setInterval`/`clearTimeout`/`clearInterval` (Fase 14).
//!
//! Antes de esto no existia NINGUN temporizador en el motor (cero
//! referencias en todo `engine-js`), y practicamente todo JavaScript real
//! los usa: carruseles, menus desplegables, `debounce` de busqueda,
//! reintentos, sondeo, y sobre todo el patron omnipresente
//! `setTimeout(inicializar, 0)` para diferir trabajo hasta despues de que
//! el documento termine de cargar. Sin ellos, la mayoria de paginas con JS
//! se quedaban a medio inicializar sin ningun error visible.
//!
//! ## Como avanza el tiempo aqui (la simplificacion que mas importa)
//!
//! Un navegador real tiene un bucle de eventos con un reloj propio: un
//! temporizador vencido se ejecuta aunque nadie toque nada. **Este motor no
//! tiene ese reloj**: los temporizadores vencidos corren cuando
//! `JsRuntime::run_due_timers` se llama, y quien la llama es
//! `core::server` despues de cada operacion real (cargar una pagina, un
//! clic, escribir, una tecla).
//!
//! Consecuencia honesta: un `setTimeout(fn, 100)` puesto durante la carga
//! SI se ejecuta (la propia carga lo dispara al terminar, y 100ms ya
//! pasaron para cuando el usuario interactua), pero un reloj que se
//! actualice solo cada segundo con la pagina quieta NO avanza hasta que el
//! usuario haga algo. Cubre el uso dominante real - diferir inicializacion,
//! reaccionar a una interaccion - no la animacion continua.
//!
//! ## Orden de ejecucion
//!
//! Los vencidos se ejecutan por `deadline` ascendente, y a igualdad de
//! `deadline` por orden de creacion (el `id`) - que es el orden real del
//! spec. Entre uno y otro se DRENAN los microtasks, igual que un navegador
//! real: cada callback de temporizador es una TAREA del bucle de eventos, y
//! al final de cada tarea se vacia la cola de microtasks (misma razon por
//! la que `JsRuntime::eval` y `dispatch_event` ya drenaban).
//!
//! ## Donde viven los callbacks
//!
//! Los datos del temporizador (id, vencimiento, intervalo) viven en Rust
//! (`TimerState`), pero la FUNCION de callback vive en un objeto JS oculto
//! (`__engineTimerCallbacks`), no en la captura de Rust. La razon es
//! concreta: capturar un `JsObject` dentro de un `NativeFunction` obliga a
//! implementar el trait `Trace` del recolector de Boa sobre una estructura
//! que ademas contiene `Instant`/`Duration` (que no son rastreables),
//! mientras que un objeto JS normal ya lo gestiona el recolector solo. El
//! coste es que ese objeto es alcanzable desde la propia pagina si la busca
//! por su nombre - se declara aqui en vez de fingir un aislamiento que no
//! hay (para eso haria falta almacenamiento del host en el `Realm`).

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsResult, JsValue, NativeFunction};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Nombre del objeto JS donde se guardan los callbacks vivos, indexados por
/// id de temporizador - ver el doc-comment del modulo para por que estan
/// ahi y no en Rust.
const CALLBACK_REGISTRY: &str = "__engineTimerCallbacks";

/// Tope de callbacks que `run_due_timers` ejecuta en una sola llamada.
/// Existe por un caso real y no teorico: un `setInterval(fn, 0)` (o un
/// `setTimeout` que se reencola a si mismo con retardo cero) esta vencido
/// otra vez en el instante en que termina, asi que sin este limite el
/// bucle no volveria nunca - el motor entero se quedaria colgado sin
/// ningun error. Con el, la pagina simplemente avanza hasta aqui en esta
/// operacion y sigue en la siguiente.
const MAX_TIMERS_PER_DRAIN: usize = 1_000;

/// Retardo minimo real de un `setInterval` (HTML spec §8.6, la regla de
/// los 4ms para temporizadores anidados). Ver `schedule` para por que se
/// aplica solo a los repetitivos.
const MIN_INTERVAL: Duration = Duration::from_millis(4);

#[derive(Debug, Clone)]
struct TimerRecord {
    id: u32,
    deadline: Instant,
    /// `Some` = `setInterval` (se reprograma tras cada disparo);
    /// `None` = `setTimeout` (dispara una vez y se borra).
    interval: Option<Duration>,
}

#[derive(Debug, Default)]
pub struct TimerState {
    next_id: u32,
    timers: Vec<TimerRecord>,
}

/// Cola de temporizadores compartida entre el `Context` de Boa (que crea y
/// cancela) y `JsRuntime::run_due_timers` (que ejecuta) - mismo patron que
/// `PendingWindowOpens` en `crate::window`.
pub type TimerQueue = Arc<Mutex<TimerState>>;

/// Envoltorio para capturar la cola dentro de un `NativeFunction`: Boa
/// exige que lo capturado implemente `Trace`, y esto no contiene NADA
/// gestionado por su recolector (los callbacks viven en JS, ver el
/// doc-comment del modulo), que es justo lo que `empty_trace!` declara.
#[derive(Clone)]
struct TimerCapture(TimerQueue);

unsafe impl boa_gc::Trace for TimerCapture {
    boa_gc::empty_trace!();
}

impl boa_gc::Finalize for TimerCapture {}

/// El retardo tal como lo interpreta un navegador real: ausente, negativo,
/// `NaN` o no numerico se tratan como cero, en vez de rechazar la llamada.
fn resolve_delay(args: &[JsValue], context: &mut Context) -> Duration {
    let ms = args
        .get(1)
        .and_then(|v| v.to_number(context).ok())
        .filter(|n| n.is_finite() && *n > 0.0)
        .unwrap_or(0.0);
    Duration::from_secs_f64(ms / 1000.0)
}

/// Cuerpo comun de `setTimeout` y `setInterval` - identicos salvo por si el
/// temporizador se reprograma tras dispararse.
fn schedule(args: &[JsValue], queue: &TimerQueue, context: &mut Context, repeating: bool) -> JsResult<JsValue> {
    let Some(callback) = args.first().and_then(JsValue::as_callable).cloned() else {
        // Sin funcion invocable no hay nada que programar. Un navegador
        // real acepta ademas una CADENA de codigo aqui (`setTimeout("...")`,
        // que evalua como `eval`) - deliberadamente NO soportado: es un
        // vector de inyeccion clasico y practicamente nadie lo usa ya.
        return Ok(JsValue::from(0));
    };
    let mut delay = resolve_delay(args, context);
    // Los intervalos se acotan a un minimo real (HTML spec §8.6: un
    // temporizador anidado con retardo menor que 4ms se sube a 4ms). Sin
    // esto, un `setInterval(fn, 0)` vuelve a estar vencido en el mismo
    // instante en que termina, asi que un solo drenado lo dispararia
    // cientos de veces seguidas hasta topar con `MAX_TIMERS_PER_DRAIN` -
    // no es un caso teorico: es lo que hace cualquier bucle de animacion
    // escrito con `setInterval`. `setTimeout` NO se acota: dispara una
    // sola vez, asi que un retardo cero ahi es exactamente lo que el
    // autor pidio (diferir hasta despues de la tarea actual).
    if repeating {
        delay = delay.max(MIN_INTERVAL);
    }

    let id = {
        let Ok(mut state) = queue.lock() else { return Ok(JsValue::from(0)) };
        state.next_id += 1;
        let id = state.next_id;
        state.timers.push(TimerRecord {
            id,
            deadline: Instant::now() + delay,
            interval: repeating.then_some(delay),
        });
        id
    };

    let registry = context
        .global_object()
        .get(js_string!(CALLBACK_REGISTRY), context)?
        .as_object()
        .cloned();
    if let Some(registry) = registry {
        registry.set(js_string!(id.to_string()), JsValue::from(callback), false, context)?;
    }

    Ok(JsValue::from(id))
}

/// Cuerpo comun de `clearTimeout` y `clearInterval` - en el spec son
/// funciones distintas por razones historicas, pero operan sobre el mismo
/// espacio de ids y son intercambiables en la practica, exactamente como
/// aqui.
fn cancel(args: &[JsValue], queue: &TimerQueue, context: &mut Context) -> JsResult<JsValue> {
    let Some(id) = args.first().and_then(|v| v.to_number(context).ok()).filter(|n| n.is_finite()) else {
        return Ok(JsValue::undefined());
    };
    let id = id as u32;

    if let Ok(mut state) = queue.lock() {
        state.timers.retain(|t| t.id != id);
    }
    let registry = context
        .global_object()
        .get(js_string!(CALLBACK_REGISTRY), context)?
        .as_object()
        .cloned();
    if let Some(registry) = registry {
        registry.delete_property_or_throw(js_string!(id.to_string()), context)?;
    }
    Ok(JsValue::undefined())
}

/// Registra los cuatro globales y devuelve la cola compartida, que
/// `JsRuntime::register_timers` guarda para poder ejecutarla despues.
pub fn register_timers(context: &mut Context) -> JsResult<TimerQueue> {
    let queue: TimerQueue = Arc::new(Mutex::new(TimerState::default()));

    let registry = ObjectInitializer::new(context).build();
    // No enumerable a proposito: un `for (var k in window)` de la pagina no
    // deberia tropezarse con el fontanero interno del motor.
    context.register_global_property(js_string!(CALLBACK_REGISTRY), registry, Attribute::empty())?;

    let set_timeout = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured, context| schedule(args, &captured.0, context, false),
        TimerCapture(queue.clone()),
    );
    let set_interval = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured, context| schedule(args, &captured.0, context, true),
        TimerCapture(queue.clone()),
    );
    let clear = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], captured, context| cancel(args, &captured.0, context),
        TimerCapture(queue.clone()),
    );

    context.register_global_builtin_callable(js_string!("setTimeout"), 2, set_timeout)?;
    context.register_global_builtin_callable(js_string!("setInterval"), 2, set_interval)?;
    context.register_global_builtin_callable(js_string!("clearTimeout"), 1, clear.clone())?;
    context.register_global_builtin_callable(js_string!("clearInterval"), 1, clear)?;

    // Colgarlos tambien de `window` si ya existe, porque muchisimo codigo
    // real escribe `window.setTimeout(...)` en vez de la forma corta.
    // Guardado igual que hace `crate::window` con `getComputedStyle`: el
    // orden de registro entre modulos no esta garantizado.
    context.eval(boa_engine::Source::from_bytes(
        b"if (typeof window !== 'undefined') { window.setTimeout = setTimeout; window.setInterval = setInterval; window.clearTimeout = clearTimeout; window.clearInterval = clearInterval; }" as &[u8],
    ))?;

    Ok(queue)
}

/// Los ids de los temporizadores YA vencidos, en orden real de ejecucion
/// (vencimiento ascendente; a igualdad, orden de creacion), reprogramando
/// de paso los `setInterval` y quitando los `setTimeout` que ya no volveran
/// a dispararse.
///
/// Devuelve solo ids: la ejecucion en si pasa FUERA, sin el cerrojo
/// tomado, porque un callback puede llamar a `setTimeout`/`clearTimeout`
/// (patron completamente normal: un temporizador que se reencola) y
/// mantener el cerrojo aqui seria un interbloqueo garantizado.
fn take_due_ids(queue: &TimerQueue, now: Instant) -> Vec<u32> {
    let Ok(mut state) = queue.lock() else { return Vec::new() };

    let mut due: Vec<(Instant, u32)> = state
        .timers
        .iter()
        .filter(|t| t.deadline <= now)
        .map(|t| (t.deadline, t.id))
        .collect();
    due.sort();

    let due_ids: Vec<u32> = due.into_iter().map(|(_, id)| id).collect();
    state.timers.retain_mut(|t| match t.interval {
        // `setInterval`: se reprograma desde AHORA, no desde su
        // vencimiento anterior - si no, un intervalo que quedo muy atrasado
        // (porque el motor estuvo un rato sin ejecutar temporizadores)
        // dispararia en rafaga todas las veces que "deberia" haber
        // disparado mientras tanto. Los navegadores reales tambien
        // colapsan esos disparos atrasados en uno.
        Some(period) if due_ids.contains(&t.id) => {
            t.deadline = now + period;
            true
        }
        // `setTimeout` ya disparado: fuera de la cola.
        None if due_ids.contains(&t.id) => false,
        _ => true,
    });

    due_ids
}

/// Ejecuta todos los temporizadores vencidos. Devuelve cuantos callbacks
/// se llegaron a invocar - `core::server` lo usa para saber si hace falta
/// rehacer el layout (un temporizador que no disparo nada no pudo haber
/// tocado el DOM).
pub fn run_due_timers(queue: &TimerQueue, context: &mut Context) -> usize {
    let mut fired = 0;

    // Bucle, no una sola pasada: un `setTimeout(fn, 0)` encolado DESDE otro
    // temporizador tambien esta vencido ya, y en un navegador real correria
    // en este mismo ciclo. `MAX_TIMERS_PER_DRAIN` es lo que impide que un
    // temporizador que se reencola sin retardo cuelgue el motor.
    while fired < MAX_TIMERS_PER_DRAIN {
        let due = take_due_ids(queue, Instant::now());
        if due.is_empty() {
            break;
        }

        for id in due {
            if fired >= MAX_TIMERS_PER_DRAIN {
                break;
            }
            let callback = context
                .global_object()
                .get(js_string!(CALLBACK_REGISTRY), context)
                .ok()
                .and_then(|r| r.as_object().cloned())
                .and_then(|registry| registry.get(js_string!(id.to_string()), context).ok())
                .and_then(|v| v.as_callable().cloned());

            let Some(callback) = callback else { continue };

            // Un error dentro de un callback NO aborta el resto (igual que
            // un navegador real, que lo deja en la consola y sigue con los
            // demas temporizadores): la pagina puede tener varios
            // independientes y que uno rompa no deberia congelar a los
            // otros.
            if let Err(error) = callback.call(&JsValue::undefined(), &[], context) {
                tracing::warn!("[timers] error dentro de un callback de temporizador: {error}");
            }
            fired += 1;

            // Cada callback de temporizador es una TAREA del bucle de
            // eventos, y al final de cada tarea se vacian los microtasks -
            // misma razon por la que `eval`/`dispatch_event` ya lo hacen.
            context.run_jobs();

            // Un `setTimeout` (no `setInterval`) ya disparado deja de tener
            // callback vivo: se borra del registro para que su funcion
            // pueda recogerse, en vez de acumularse durante toda la vida de
            // la pagina.
            let still_scheduled = queue.lock().is_ok_and(|state| state.timers.iter().any(|t| t.id == id));
            if !still_scheduled {
                if let Ok(Some(registry)) = context.global_object().get(js_string!(CALLBACK_REGISTRY), context).map(|r| r.as_object().cloned()) {
                    let _ = registry.delete_property_or_throw(js_string!(id.to_string()), context);
                }
            }
        }
    }

    fired
}

#[cfg(test)]
mod tests {
    use crate::runtime::JsRuntime;

    fn runtime_with_timers() -> JsRuntime {
        let mut runtime = JsRuntime::new();
        runtime.register_timers().expect("los temporizadores deberian registrarse");
        runtime
    }

    #[test]
    fn set_timeout_does_not_run_the_callback_immediately() {
        let mut runtime = runtime_with_timers();
        runtime.eval("var corrio = false; setTimeout(function () { corrio = true; }, 0);").expect("no deberia lanzar");
        assert_eq!(
            runtime.eval("corrio").unwrap(),
            "false",
            "el callback NO deberia correr de forma sincrona - eso era justo el bug que tenia queueMicrotask antes"
        );
    }

    #[test]
    fn a_zero_delay_timeout_runs_on_the_next_drain() {
        let mut runtime = runtime_with_timers();
        runtime.eval("var corrio = false; setTimeout(function () { corrio = true; }, 0);").expect("no deberia lanzar");
        assert_eq!(runtime.run_due_timers(), 1);
        assert_eq!(runtime.eval("corrio").unwrap(), "true");
    }

    #[test]
    fn a_timer_that_has_not_come_due_yet_does_not_run() {
        let mut runtime = runtime_with_timers();
        runtime.eval("var corrio = false; setTimeout(function () { corrio = true; }, 60000);").expect("no deberia lanzar");
        assert_eq!(runtime.run_due_timers(), 0, "un temporizador a 60s no deberia dispararse ya");
        assert_eq!(runtime.eval("corrio").unwrap(), "false");
    }

    #[test]
    fn timers_run_in_deadline_order_not_in_creation_order() {
        let mut runtime = runtime_with_timers();
        runtime
            .eval("var orden = []; setTimeout(function(){orden.push('segundo');}, 20); setTimeout(function(){orden.push('primero');}, 0);")
            .expect("no deberia lanzar");
        std::thread::sleep(std::time::Duration::from_millis(40));
        runtime.run_due_timers();
        assert_eq!(runtime.eval("orden.join(',')").unwrap(), "\"primero,segundo\"", "deberia mandar el vencimiento, no el orden de creacion");
    }

    #[test]
    fn two_timers_with_the_same_delay_run_in_creation_order() {
        let mut runtime = runtime_with_timers();
        runtime
            .eval("var orden = []; setTimeout(function(){orden.push('a');}, 0); setTimeout(function(){orden.push('b');}, 0);")
            .expect("no deberia lanzar");
        runtime.run_due_timers();
        assert_eq!(runtime.eval("orden.join(',')").unwrap(), "\"a,b\"");
    }

    #[test]
    fn clear_timeout_prevents_the_callback_from_ever_running() {
        let mut runtime = runtime_with_timers();
        runtime
            .eval("var corrio = false; var id = setTimeout(function(){corrio = true;}, 0); clearTimeout(id);")
            .expect("no deberia lanzar");
        assert_eq!(runtime.run_due_timers(), 0);
        assert_eq!(runtime.eval("corrio").unwrap(), "false");
    }

    #[test]
    fn a_timeout_fires_only_once_even_across_several_drains() {
        let mut runtime = runtime_with_timers();
        runtime.eval("var veces = 0; setTimeout(function(){veces++;}, 0);").expect("no deberia lanzar");
        runtime.run_due_timers();
        runtime.run_due_timers();
        runtime.run_due_timers();
        assert_eq!(runtime.eval("veces").unwrap(), "1", "un setTimeout deberia dispararse UNA vez, no en cada drenado");
    }

    #[test]
    fn set_interval_fires_again_on_a_later_drain() {
        let mut runtime = runtime_with_timers();
        runtime.eval("var veces = 0; setInterval(function(){veces++;}, 10);").expect("no deberia lanzar");
        std::thread::sleep(std::time::Duration::from_millis(20));
        runtime.run_due_timers();
        assert_eq!(runtime.eval("veces").unwrap(), "1");
        std::thread::sleep(std::time::Duration::from_millis(20));
        runtime.run_due_timers();
        assert_eq!(runtime.eval("veces").unwrap(), "2", "un setInterval deberia volver a dispararse, a diferencia de setTimeout");
    }

    #[test]
    fn clear_interval_stops_a_repeating_timer() {
        let mut runtime = runtime_with_timers();
        runtime.eval("var veces = 0; var id = setInterval(function(){veces++;}, 10);").expect("no deberia lanzar");
        std::thread::sleep(std::time::Duration::from_millis(20));
        runtime.run_due_timers();
        assert_eq!(runtime.eval("veces").unwrap(), "1");

        runtime.eval("clearInterval(id)").expect("no deberia lanzar");
        std::thread::sleep(std::time::Duration::from_millis(20));
        runtime.run_due_timers();
        assert_eq!(runtime.eval("veces").unwrap(), "1", "tras clearInterval no deberia volver a dispararse");
    }

    /// La regla de los 4ms del spec, que este motor necesita ademas por
    /// una razon propia: sin ella un `setInterval(fn, 0)` vuelve a estar
    /// vencido en el instante en que termina, asi que un SOLO drenado lo
    /// dispararia cientos de veces hasta topar con el limite - encontrado
    /// justo asi, con un test que fallo al escribir esta fase.
    #[test]
    fn a_zero_delay_interval_is_clamped_instead_of_firing_hundreds_of_times_in_one_drain() {
        let mut runtime = runtime_with_timers();
        runtime.eval("var veces = 0; setInterval(function(){veces++;}, 0);").expect("no deberia lanzar");
        // El acotado lo empuja 4ms al futuro, asi que hay que esperar de
        // verdad - un drenado inmediato no dispararia nada todavia, que es
        // en si mismo la prueba de que el acotado se aplico.
        assert_eq!(runtime.run_due_timers(), 0, "recien creado no deberia estar vencido: el acotado a 4ms lo empujo al futuro");

        std::thread::sleep(std::time::Duration::from_millis(10));
        let fired = runtime.run_due_timers();
        assert_eq!(fired, 1, "deberia dispararse una sola vez por drenado, no en bucle hasta el tope");
    }

    /// El complementario del anterior: `setTimeout` con retardo cero NO se
    /// acota, porque dispara una sola vez y diferir hasta despues de la
    /// tarea actual es exactamente lo que el autor pidio.
    #[test]
    fn a_zero_delay_timeout_is_not_clamped_and_runs_on_the_very_next_drain() {
        let mut runtime = runtime_with_timers();
        runtime.eval("var corrio = false; setTimeout(function(){corrio = true;}, 0);").expect("no deberia lanzar");
        runtime.run_due_timers();
        assert_eq!(runtime.eval("corrio").unwrap(), "true", "sin espera ninguna, a diferencia de un setInterval de retardo cero");
    }

    /// El patron real de "reencolarse a si mismo" (una animacion o un
    /// sondeo escritos con setTimeout en vez de setInterval).
    #[test]
    fn a_timer_that_reschedules_itself_keeps_running_across_drains() {
        let mut runtime = runtime_with_timers();
        runtime
            .eval("var veces = 0; function tic(){ veces++; if (veces < 3) setTimeout(tic, 0); } setTimeout(tic, 0);")
            .expect("no deberia lanzar");
        runtime.run_due_timers();
        assert_eq!(runtime.eval("veces").unwrap(), "3", "los reencolados con retardo cero deberian correr en el mismo ciclo, como en un navegador real");
    }

    /// El limite que impide colgar el motor: un temporizador que se
    /// reencola SIN condicion de parada.
    #[test]
    fn an_endlessly_rescheduling_timer_is_capped_instead_of_hanging_the_engine() {
        let mut runtime = runtime_with_timers();
        runtime
            .eval("var veces = 0; function bucle(){ veces++; setTimeout(bucle, 0); } setTimeout(bucle, 0);")
            .expect("no deberia lanzar");
        let fired = runtime.run_due_timers();
        assert_eq!(fired, super::MAX_TIMERS_PER_DRAIN, "deberia parar en el tope en vez de no volver nunca");
    }

    #[test]
    fn an_error_inside_one_callback_does_not_stop_the_others() {
        let mut runtime = runtime_with_timers();
        runtime
            .eval("var ok = false; setTimeout(function(){ throw new Error('fallo'); }, 0); setTimeout(function(){ ok = true; }, 0);")
            .expect("no deberia lanzar");
        runtime.run_due_timers();
        assert_eq!(runtime.eval("ok").unwrap(), "true", "un callback que lanza no deberia impedir que corran los demas");
    }

    #[test]
    fn microtasks_queued_by_a_timer_callback_drain_before_the_call_returns() {
        let mut runtime = runtime_with_timers();
        runtime
            .eval("var orden = []; setTimeout(function(){ orden.push('temporizador'); queueMicrotask(function(){ orden.push('microtask'); }); }, 0);")
            .expect("no deberia lanzar");
        runtime.run_due_timers();
        assert_eq!(
            runtime.eval("orden.join(',')").unwrap(),
            "\"temporizador,microtask\"",
            "cada callback es una tarea, y al final de cada tarea se vacian los microtasks"
        );
    }

    #[test]
    fn set_timeout_returns_a_usable_nonzero_id() {
        let mut runtime = runtime_with_timers();
        let id = runtime.eval("setTimeout(function(){}, 1000)").expect("no deberia lanzar");
        assert_ne!(id, "0", "deberia devolver un id real que clearTimeout pueda usar");
    }

    #[test]
    fn a_missing_or_non_callable_argument_does_nothing_instead_of_throwing() {
        let mut runtime = runtime_with_timers();
        assert_eq!(runtime.eval("setTimeout(); setTimeout(42); 'sin romper'").unwrap(), "\"sin romper\"");
        assert_eq!(runtime.run_due_timers(), 0);
    }

    #[test]
    fn a_negative_or_missing_delay_is_treated_as_zero_like_a_real_browser() {
        let mut runtime = runtime_with_timers();
        runtime.eval("var n = 0; setTimeout(function(){n++;}); setTimeout(function(){n++;}, -5);").expect("no deberia lanzar");
        runtime.run_due_timers();
        assert_eq!(runtime.eval("n").unwrap(), "2", "sin retardo y con retardo negativo deberian comportarse como cero");
    }

    /// Sin `register_timers`, los globales no existen - mismo criterio
    /// honesto que `fetch`/`window` (ver `JsRuntime::register_fetch`).
    #[test]
    fn timers_are_not_defined_at_all_unless_they_were_registered() {
        let mut runtime = JsRuntime::new();
        assert_eq!(runtime.eval("typeof setTimeout").unwrap(), "\"undefined\"");
    }
}
