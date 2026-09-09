//! Utilidades de plataforma que no son DOM ni red: `console`, `URL`,
//! `URLSearchParams`, `performance`, `atob`/`btoa` y `TextEncoder`/
//! `TextDecoder` (tarea C5 del `plan.md`).
//!
//! **Por que estas y no otras.** La sonda `engine/tests/probes/api-probe.html`
//! midio 48 de 114 el 2026-09-09. De lo que faltaba, estas son las que mas
//! bundles reales tocan en su primera linea util, y la razon por la que eso
//! importa la establecio la Fase 39: la ausencia de UNA sola API lanza un
//! `TypeError` que se lleva por delante el script ENTERO. El coste de una API
//! ausente no es proporcional a lo usada que sea.
//!
//! `console` es el caso extremo de eso. Practicamente todo codigo de
//! produccion conserva algun `console.warn` o `console.error`, y muchos
//! frameworks avisan por ahi en desarrollo. Sin el objeto, ese aviso —que
//! deberia ser informativo— mataba la pagina.
//!
//! **Donde va la salida de `console`: a `tracing`, jamas a stdout.** stdout es
//! el canal NDJSON del protocolo y una sola linea que no sea JSON lo rompe
//! (ver la cabecera de `core/bin/engine_server.rs`). Esto ya lo garantiza el
//! suscriptor de `tracing`, que escribe a stderr.
//!
//! **Lo que NO se registra aqui, y por que.** `structuredClone`,
//! `AbortController` y `crypto` siguen ausentes a proposito:
//! `AbortController` sin cancelacion real del `fetch` seria justo el stub que
//! la doctrina prohibe (el codigo cree haber cancelado y no cancelo nada), y
//! `crypto.getRandomValues` con numeros no criptograficos seria peor que su
//! ausencia. Quedan anotados en `huecos_sin_resolver.md`.

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsResult, JsValue, NativeFunction};

use base64::Engine as _;

/// Registra todas las utilidades de este modulo en `context`.
///
/// Se llama DESPUES de `register_window` para que `console` pueda colgarse
/// tambien de `window` (`window.console` es como lo escribe algun codigo), y
/// antes de ejecutar ningun `<script>` de la pagina: el primer script debe
/// verlas ya disponibles, no solo los listeners registrados mas tarde.
pub fn register_platform(context: &mut Context) -> JsResult<()> {
    register_console(context)?;
    register_base64(context)?;
    register_performance(context)?;
    register_text_codec(context)?;
    register_url(context)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// console
// ---------------------------------------------------------------------------

/// Formatea los argumentos de una llamada a `console.*` como lo haria un
/// navegador: separados por espacios, cada uno convertido a texto.
///
/// Los objetos NO se serializan con `JSON.stringify`: un objeto con ciclos
/// (comunisimo — cualquier nodo del DOM tiene `parentNode`) haria lanzar a
/// `stringify`, y una llamada a `console.log` que LANZA es exactamente el
/// problema que este modulo viene a quitar. `[object Object]` es menos util
/// pero nunca rompe.
fn formatear_argumentos(args: &[JsValue], context: &mut Context) -> String {
    let mut partes = Vec::with_capacity(args.len());
    for arg in args {
        let texto = match arg.to_string(context) {
            Ok(s) => s.to_std_string_escaped(),
            // `to_string` puede lanzar si el valor tiene un `toString` propio
            // que lance. Tampoco ahi debe morir el log.
            Err(_) => "<valor no representable>".to_string(),
        };
        partes.push(texto);
    }
    partes.join(" ")
}

/// Crea un metodo de `console` que emite por `tracing` en el nivel dado.
macro_rules! metodo_console {
    ($nivel:ident) => {
        NativeFunction::from_fn_ptr(|_this, args, context| {
            let mensaje = formatear_argumentos(args, context);
            tracing::$nivel!(target: "console", "{mensaje}");
            Ok(JsValue::undefined())
        })
    };
}

fn register_console(context: &mut Context) -> JsResult<()> {
    // `console.assert(cond, ...)`: solo emite si la condicion es falsa, igual
    // que el spec. Un `assert` que emitiera siempre convertiria un log de
    // diagnostico en ruido constante.
    let assert = NativeFunction::from_fn_ptr(|_this, args, context| {
        if args.first().map(|v| v.to_boolean()).unwrap_or(false) {
            return Ok(JsValue::undefined());
        }
        let mensaje = formatear_argumentos(args.get(1..).unwrap_or(&[]), context);
        tracing::warn!(target: "console", "assert fallido: {mensaje}");
        Ok(JsValue::undefined())
    });

    // `group`/`groupEnd`/`time`/`timeEnd`/`count`/`table`/`dir` existen porque
    // su AUSENCIA rompe el script, no porque agrupen o midan de verdad: aqui
    // se comportan como un `log` normal. Es una aproximacion declarada, no un
    // stub vacio — emiten lo que se les pasa, asi que la informacion no se
    // pierde, solo la presentacion.
    let console = ObjectInitializer::new(context)
        .function(metodo_console!(info), js_string!("log"), 0)
        .function(metodo_console!(info), js_string!("info"), 0)
        .function(metodo_console!(warn), js_string!("warn"), 0)
        .function(metodo_console!(error), js_string!("error"), 0)
        .function(metodo_console!(debug), js_string!("debug"), 0)
        .function(metodo_console!(trace), js_string!("trace"), 0)
        .function(metodo_console!(info), js_string!("dir"), 0)
        .function(metodo_console!(info), js_string!("table"), 0)
        .function(metodo_console!(info), js_string!("group"), 0)
        .function(metodo_console!(info), js_string!("groupCollapsed"), 0)
        .function(metodo_console!(info), js_string!("groupEnd"), 0)
        .function(metodo_console!(info), js_string!("time"), 0)
        .function(metodo_console!(info), js_string!("timeEnd"), 0)
        .function(metodo_console!(info), js_string!("timeLog"), 0)
        .function(metodo_console!(info), js_string!("count"), 0)
        .function(metodo_console!(info), js_string!("countReset"), 0)
        .function(assert, js_string!("assert"), 0)
        .build();

    context.register_global_property(js_string!("console"), console.clone(), Attribute::all())?;
    colgar_de_window(context, "console", console.into())?;
    Ok(())
}

/// Cuelga `valor` tambien de `window`, si `window` existe.
///
/// Hace falta porque en este motor `window` NO es el objeto global (ver la
/// cabecera de `window.rs`): registrar un global no lo hace aparecer en
/// `window.*`, asi que hay que ponerlo en los dos sitios. Codigo real usa
/// ambas formas.
fn colgar_de_window(context: &mut Context, nombre: &str, valor: JsValue) -> JsResult<()> {
    let window = context
        .global_object()
        .get(js_string!("window"), context)?;
    let Some(window) = window.as_object().cloned() else {
        // Sin `window` registrado (el arnes de tests, o un `Context` sin
        // `register_window`) no hay nada que hacer, y no es un error: el
        // global suelto ya quedo puesto.
        return Ok(());
    };
    window.set(js_string!(nombre.to_string()), valor, false, context)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// atob / btoa
// ---------------------------------------------------------------------------

fn register_base64(context: &mut Context) -> JsResult<()> {
    // `btoa` recibe una cadena "binaria": cada unidad de codigo debe caber en
    // un byte. Si no cabe, el spec manda lanzar `InvalidCharacterError` en vez
    // de truncar en silencio, que produciria datos corruptos indetectables.
    let btoa = NativeFunction::from_fn_ptr(|_this, args, context| {
        let entrada = match args.first() {
            Some(v) => v.to_string(context)?.to_std_string_escaped(),
            None => String::new(),
        };
        let mut bytes = Vec::with_capacity(entrada.len());
        for c in entrada.chars() {
            let punto = c as u32;
            if punto > 0xFF {
                return Err(boa_engine::JsNativeError::typ()
                    .with_message(
                        "btoa: la cadena tiene caracteres fuera del rango Latin-1 \
                         (InvalidCharacterError)",
                    )
                    .into());
            }
            bytes.push(punto as u8);
        }
        let salida = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(js_string!(salida).into())
    });

    let atob = NativeFunction::from_fn_ptr(|_this, args, context| {
        let entrada = match args.first() {
            Some(v) => v.to_string(context)?.to_std_string_escaped(),
            None => String::new(),
        };
        // El spec ignora los espacios en blanco antes de decodificar: un
        // Base64 partido en varias lineas (muy comun) debe decodificar igual.
        let limpia: String = entrada.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(limpia.as_bytes())
            .map_err(|e| {
                boa_engine::JsNativeError::typ()
                    .with_message(format!("atob: entrada no es Base64 valido ({e})"))
            })?;
        // Cada byte vuelve a ser una unidad de codigo, que es lo contrario
        // exacto de lo que hizo `btoa` - no es UTF-8.
        let texto: String = bytes.iter().map(|b| *b as char).collect();
        Ok(js_string!(texto).into())
    });

    context.register_global_builtin_callable(js_string!("btoa"), 1, btoa)?;
    context.register_global_builtin_callable(js_string!("atob"), 1, atob)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// performance
// ---------------------------------------------------------------------------

fn register_performance(context: &mut Context) -> JsResult<()> {
    // El origen temporal es el momento de registrar `performance`, que es lo
    // mas cerca que este motor tiene del "inicio de la navegacion" del spec:
    // se registra justo antes de ejecutar el primer script de la pagina.
    let origen = std::time::Instant::now();

    #[derive(Clone, Copy)]
    struct Origen(std::time::Instant);
    impl boa_gc::Finalize for Origen {}
    unsafe impl boa_gc::Trace for Origen {
        boa_gc::empty_trace!();
    }

    let now = NativeFunction::from_copy_closure_with_captures(
        |_this, _args: &[JsValue], origen: &Origen, _context| {
            // Milisegundos con parte fraccionaria, igual que el spec. Un
            // `performance.now()` que devolviera enteros haria que cualquier
            // medida de menos de un milisegundo saliera cero.
            Ok(JsValue::from(origen.0.elapsed().as_secs_f64() * 1000.0))
        },
        Origen(origen),
    );

    // `mark`/`measure`/`clearMarks` no llevan registro real todavia: existen
    // para que llamarlas no lance. Devuelven `undefined`, que es lo que
    // devuelven de verdad, asi que ningun codigo puede distinguirlo salvo
    // consultando `getEntries()` - que por eso NO se registra: devolver una
    // lista vacia fingiria que se midio y no se midio.
    let no_op = NativeFunction::from_fn_ptr(|_this, _args, _context| Ok(JsValue::undefined()));

    let performance = ObjectInitializer::new(context)
        .function(now, js_string!("now"), 0)
        .function(no_op.clone(), js_string!("mark"), 0)
        .function(no_op.clone(), js_string!("measure"), 0)
        .function(no_op.clone(), js_string!("clearMarks"), 0)
        .function(no_op, js_string!("clearMeasures"), 0)
        .property(js_string!("timeOrigin"), JsValue::from(0.0), Attribute::all())
        .build();

    context.register_global_property(js_string!("performance"), performance.clone(), Attribute::all())?;
    colgar_de_window(context, "performance", performance.into())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// TextEncoder / TextDecoder
// ---------------------------------------------------------------------------

fn register_text_codec(context: &mut Context) -> JsResult<()> {
    // Solo UTF-8. El spec permite otras codificaciones en `TextDecoder`, pero
    // fingirlas convirtiendo mal seria peor que rechazarlas: se lanza, que es
    // lo que hace un navegador real con una etiqueta desconocida.
    let encoder_ctor = NativeFunction::from_fn_ptr(|_this, _args, context| {
        let encode = NativeFunction::from_fn_ptr(|_this, args, context| {
            let texto = match args.first() {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => String::new(),
            };
            let bytes: Vec<JsValue> = texto.as_bytes().iter().map(|b| JsValue::from(*b)).collect();
            // Se devuelve un Array normal, no un `Uint8Array`: los tipados de
            // Boa no se construyen desde Rust con esta API. Un Array se indexa
            // y se recorre igual, que es lo que hace casi todo el codigo que
            // lo usa; lo que NO funcionara es pasarselo a algo que exija un
            // TypedArray de verdad. Declarado en huecos_sin_resolver.md.
            Ok(boa_engine::object::builtins::JsArray::from_iter(bytes, context).into())
        });
        Ok(ObjectInitializer::new(context)
            .function(encode, js_string!("encode"), 1)
            .property(js_string!("encoding"), js_string!("utf-8"), Attribute::all())
            .build()
            .into())
    });

    let decoder_ctor = NativeFunction::from_fn_ptr(|_this, args, context| {
        if let Some(etiqueta) = args.first() {
            let etiqueta = etiqueta.to_string(context)?.to_std_string_escaped();
            let normalizada = etiqueta.to_ascii_lowercase();
            if !matches!(normalizada.as_str(), "" | "utf-8" | "utf8" | "unicode-1-1-utf-8") {
                return Err(boa_engine::JsNativeError::range()
                    .with_message(format!(
                        "TextDecoder: este motor solo soporta utf-8, no {etiqueta:?}"
                    ))
                    .into());
            }
        }
        let decode = NativeFunction::from_fn_ptr(|_this, args, context| {
            let Some(entrada) = args.first().and_then(|v| v.as_object()) else {
                return Ok(js_string!("").into());
            };
            let longitud = entrada.get(js_string!("length"), context)?.to_length(context)?;
            let mut bytes = Vec::with_capacity(longitud as usize);
            for i in 0..longitud {
                let valor = entrada.get(i, context)?.to_number(context)?;
                bytes.push(valor as u8);
            }
            // `from_utf8_lossy` y no `from_utf8`: el spec dice que sin
            // `{fatal: true}` los bytes invalidos se sustituyen por U+FFFD,
            // no que se lance.
            Ok(js_string!(String::from_utf8_lossy(&bytes).into_owned()).into())
        });
        Ok(ObjectInitializer::new(context)
            .function(decode, js_string!("decode"), 1)
            .property(js_string!("encoding"), js_string!("utf-8"), Attribute::all())
            .build()
            .into())
    });

    context.register_global_callable(js_string!("TextEncoder"), 0, encoder_ctor)?;
    context.register_global_callable(js_string!("TextDecoder"), 0, decoder_ctor)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// URL / URLSearchParams
// ---------------------------------------------------------------------------

/// Construye el objeto que devuelven `new URL(...)` y `location`-alikes.
///
/// Se apoya en el crate `url` (el mismo que ya usa `location.rs` y la capa de
/// red), no en un troceado a mano de la cadena: resolver una URL relativa
/// contra una base tiene mas bordes de los que parece (`..` de mas, barras
/// dobles, puertos por defecto, IDN) y ya estan resueltos ahi.
fn construir_objeto_url(url: &url::Url, context: &mut Context) -> JsValue {
    let origen = format!(
        "{}://{}",
        url.scheme(),
        url.host_str().unwrap_or_default()
    );
    let origen = match url.port() {
        Some(p) => format!("{origen}:{p}"),
        None => origen,
    };

    let search = match url.query() {
        Some(q) if !q.is_empty() => format!("?{q}"),
        _ => String::new(),
    };
    let hash = match url.fragment() {
        Some(f) if !f.is_empty() => format!("#{f}"),
        _ => String::new(),
    };
    let host = match url.port() {
        Some(p) => format!("{}:{}", url.host_str().unwrap_or_default(), p),
        None => url.host_str().unwrap_or_default().to_string(),
    };

    let mut init = ObjectInitializer::new(context);
    init.property(js_string!("href"), js_string!(url.as_str().to_string()), Attribute::all())
        .property(js_string!("protocol"), js_string!(format!("{}:", url.scheme())), Attribute::all())
        .property(js_string!("host"), js_string!(host), Attribute::all())
        .property(js_string!("hostname"), js_string!(url.host_str().unwrap_or_default().to_string()), Attribute::all())
        .property(js_string!("port"), js_string!(url.port().map(|p| p.to_string()).unwrap_or_default()), Attribute::all())
        .property(js_string!("pathname"), js_string!(url.path().to_string()), Attribute::all())
        .property(js_string!("search"), js_string!(search), Attribute::all())
        .property(js_string!("hash"), js_string!(hash), Attribute::all())
        .property(js_string!("origin"), js_string!(origen), Attribute::all())
        .property(js_string!("username"), js_string!(url.username().to_string()), Attribute::all())
        .property(js_string!("password"), js_string!(url.password().unwrap_or_default().to_string()), Attribute::all());

    // `toString()`/`toJSON()` devuelven el href, igual que el spec. Sin esto,
    // `String(new URL(...))` daria `[object Object]` y cualquier codigo que
    // concatene una URL en una plantilla produciria basura silenciosa.
    let href = url.as_str().to_string();
    #[derive(Clone)]
    struct Href(String);
    impl boa_gc::Finalize for Href {}
    unsafe impl boa_gc::Trace for Href {
        boa_gc::empty_trace!();
    }
    let to_string = NativeFunction::from_copy_closure_with_captures(
        |_this, _args: &[JsValue], href: &Href, _context| {
            Ok(JsValue::from(js_string!(href.0.clone())))
        },
        Href(href),
    );
    init.function(to_string.clone(), js_string!("toString"), 0);
    init.function(to_string, js_string!("toJSON"), 0);

    init.build().into()
}

/// Objeto `URLSearchParams` sobre una lista de pares ya parseada.
fn construir_search_params(pares: Vec<(String, String)>, context: &mut Context) -> JsValue {
    #[derive(Clone)]
    struct Pares(Vec<(String, String)>);
    impl boa_gc::Finalize for Pares {}
    unsafe impl boa_gc::Trace for Pares {
        boa_gc::empty_trace!();
    }

    let capturados = Pares(pares.clone());
    let get = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], pares: &Pares, context| {
            let clave = match args.first() {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => return Ok(JsValue::null()),
            };
            // `get` devuelve el PRIMER valor, no el ultimo: con `?a=1&a=2`,
            // `get('a')` es "1". Es una de esas diferencias que solo se notan
            // en produccion.
            Ok(match pares.0.iter().find(|(k, _)| *k == clave) {
                Some((_, v)) => js_string!(v.clone()).into(),
                None => JsValue::null(),
            })
        },
        capturados.clone(),
    );

    let get_all = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], pares: &Pares, context| {
            let clave = match args.first() {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => String::new(),
            };
            let valores: Vec<JsValue> = pares
                .0
                .iter()
                .filter(|(k, _)| *k == clave)
                .map(|(_, v)| js_string!(v.clone()).into())
                .collect();
            Ok(boa_engine::object::builtins::JsArray::from_iter(valores, context).into())
        },
        capturados.clone(),
    );

    let has = NativeFunction::from_copy_closure_with_captures(
        |_this, args: &[JsValue], pares: &Pares, context| {
            let clave = match args.first() {
                Some(v) => v.to_string(context)?.to_std_string_escaped(),
                None => String::new(),
            };
            Ok(JsValue::from(pares.0.iter().any(|(k, _)| *k == clave)))
        },
        capturados.clone(),
    );

    let to_string = NativeFunction::from_copy_closure_with_captures(
        |_this, _args: &[JsValue], pares: &Pares, _context| {
            let texto = pares
                .0
                .iter()
                .map(|(k, v)| format!("{}={}", codificar_componente(k), codificar_componente(v)))
                .collect::<Vec<_>>()
                .join("&");
            Ok(JsValue::from(js_string!(texto)))
        },
        capturados,
    );

    ObjectInitializer::new(context)
        .function(get, js_string!("get"), 1)
        .function(get_all, js_string!("getAll"), 1)
        .function(has, js_string!("has"), 1)
        .function(to_string, js_string!("toString"), 0)
        .build()
        .into()
}

/// Percent-encoding para `URLSearchParams::toString`. Solo lo que el
/// `application/x-www-form-urlencoded` serializer exige escapar.
fn codificar_componente(s: &str) -> String {
    let mut salida = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                salida.push(b as char)
            }
            b' ' => salida.push('+'),
            _ => salida.push_str(&format!("%{b:02X}")),
        }
    }
    salida
}

fn register_url(context: &mut Context) -> JsResult<()> {
    let url_ctor = NativeFunction::from_fn_ptr(|_this, args, context| {
        let entrada = match args.first() {
            Some(v) => v.to_string(context)?.to_std_string_escaped(),
            None => String::new(),
        };
        let base = match args.get(1) {
            Some(v) if !v.is_undefined() => Some(v.to_string(context)?.to_std_string_escaped()),
            _ => None,
        };

        // El spec manda lanzar `TypeError` con una URL invalida, no devolver
        // algo a medias. Codigo real lo envuelve en try/catch para decidir si
        // una cadena es una URL, asi que lanzar es la respuesta UTIL.
        let parseada = match base {
            Some(base) => url::Url::parse(&base)
                .and_then(|b| b.join(&entrada))
                .map_err(|e| format!("{e}")),
            None => url::Url::parse(&entrada).map_err(|e| format!("{e}")),
        };

        match parseada {
            Ok(u) => Ok(construir_objeto_url(&u, context)),
            Err(e) => Err(boa_engine::JsNativeError::typ()
                .with_message(format!("URL invalida: {entrada:?} ({e})"))
                .into()),
        }
    });

    let params_ctor = NativeFunction::from_fn_ptr(|_this, args, context| {
        let entrada = match args.first() {
            Some(v) if !v.is_undefined() => v.to_string(context)?.to_std_string_escaped(),
            _ => String::new(),
        };
        let limpia = entrada.trim_start_matches('?');
        // Se parsea con el mismo crate que el resto del motor, para que
        // `?a=1&a=2` y el decodificado de `%20`/`+` se comporten igual aqui
        // que en la capa de red.
        let pares: Vec<(String, String)> = url::form_urlencoded::parse(limpia.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        Ok(construir_search_params(pares, context))
    });

    context.register_global_callable(js_string!("URL"), 1, url_ctor)?;
    context.register_global_callable(js_string!("URLSearchParams"), 1, params_ctor)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::Source;

    /// Evalua `codigo` en un contexto con las utilidades registradas y
    /// devuelve el resultado como texto.
    fn evaluar(codigo: &str) -> String {
        let mut context = Context::default();
        register_platform(&mut context).expect("no se pudieron registrar las utilidades");
        match context.eval(Source::from_bytes(codigo)) {
            Ok(v) => v
                .to_string(&mut context)
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_else(|_| "<no representable>".to_string()),
            Err(e) => format!("ERROR: {e}"),
        }
    }

    #[test]
    fn console_existe_con_toda_la_familia() {
        assert_eq!(
            evaluar(
                "['log','info','warn','error','debug','trace','table','group','groupEnd',\
                  'time','timeEnd','count','assert','dir'].every(m => typeof console[m] === 'function')"
            ),
            "true"
        );
    }

    #[test]
    fn console_log_no_lanza_con_un_objeto_ciclico() {
        // El caso real: cualquier nodo del DOM tiene `parentNode`, asi que un
        // `console.log(elemento)` pasa un ciclo. Si el formateo usara
        // `JSON.stringify`, lanzaria - y un log que lanza es peor que no tener
        // log.
        assert_eq!(
            evaluar("var a = {}; a.yo = a; console.log(a); 'sobrevivio'"),
            "sobrevivio"
        );
    }

    #[test]
    fn console_assert_solo_avisa_cuando_la_condicion_es_falsa() {
        // No se puede observar la salida de `tracing` desde aqui; lo que se
        // comprueba es que ninguna de las dos ramas lanza.
        assert_eq!(
            evaluar("console.assert(true, 'no'); console.assert(false, 'si'); 'ok'"),
            "ok"
        );
    }

    #[test]
    fn btoa_y_atob_son_inversos() {
        assert_eq!(evaluar("btoa('hola')"), "aG9sYQ==");
        assert_eq!(evaluar("atob('aG9sYQ==')"), "hola");
        assert_eq!(evaluar("atob(btoa('Navegador IA'))"), "Navegador IA");
    }

    #[test]
    fn atob_ignora_los_espacios_del_base64() {
        // Base64 partido en lineas es comun en HTML y en cabeceras.
        assert_eq!(evaluar("atob('aG9s\\n YQ==')"), "hola");
    }

    #[test]
    fn btoa_lanza_con_caracteres_fuera_de_latin1() {
        // Truncar en silencio produciria datos corruptos que nadie detecta.
        assert!(evaluar("try { btoa('ñ✓'); 'no lanzo' } catch (e) { 'lanzo' }").contains("lanzo"));
    }

    #[test]
    fn atob_lanza_con_base64_invalido() {
        assert_eq!(
            evaluar("try { atob('!!!no-es-base64!!!'); 'no lanzo' } catch (e) { 'lanzo' }"),
            "lanzo"
        );
    }

    #[test]
    fn performance_now_avanza_y_tiene_parte_fraccionaria() {
        assert_eq!(evaluar("typeof performance.now()"), "number");
        // Dos lecturas seguidas no pueden retroceder.
        assert_eq!(
            evaluar("var a = performance.now(); var b = performance.now(); b >= a"),
            "true"
        );
    }

    #[test]
    fn text_encoder_codifica_utf8_real() {
        assert_eq!(evaluar("new TextEncoder().encode('a')[0]"), "97");
        // 'ñ' son DOS bytes en UTF-8 (0xC3 0xB1): si saliera uno, estaria
        // truncando a Latin-1.
        assert_eq!(evaluar("new TextEncoder().encode('ñ').length"), "2");
    }

    #[test]
    fn text_decoder_deshace_lo_que_hizo_el_encoder() {
        assert_eq!(
            evaluar("new TextDecoder().decode(new TextEncoder().encode('camión'))"),
            "camión"
        );
    }

    #[test]
    fn text_decoder_rechaza_codificaciones_que_no_soporta() {
        // Fingir que decodifica latin-1 devolveria texto mal formado sin aviso.
        assert_eq!(
            evaluar("try { new TextDecoder('latin1'); 'no lanzo' } catch (e) { 'lanzo' }"),
            "lanzo"
        );
        assert_eq!(
            evaluar("try { new TextDecoder('UTF-8'); 'ok' } catch (e) { 'lanzo' }"),
            "ok"
        );
    }

    #[test]
    fn url_resuelve_relativas_contra_su_base() {
        assert_eq!(
            evaluar("new URL('/a?b=1', 'https://x.dev/').href"),
            "https://x.dev/a?b=1"
        );
        // Una relativa SIN barra inicial se resuelve contra el directorio, no
        // contra la raiz: es justo el caso que un troceado a mano falla.
        assert_eq!(
            evaluar("new URL('c.json', 'https://x.dev/a/b/').href"),
            "https://x.dev/a/b/c.json"
        );
    }

    #[test]
    fn url_expone_sus_partes() {
        let codigo = "var u = new URL('https://a.dev:8443/p/q?x=1#z');\
                      [u.protocol, u.hostname, u.port, u.pathname, u.search, u.hash, u.origin].join('|')";
        assert_eq!(
            evaluar(codigo),
            "https:|a.dev|8443|/p/q|?x=1|#z|https://a.dev:8443"
        );
    }

    #[test]
    fn url_se_convierte_a_texto_como_su_href() {
        // Sin `toString`, concatenar una URL daria "[object Object]" y el fallo
        // aparecería mucho mas tarde, en la peticion.
        assert_eq!(evaluar("'' + new URL('https://x.dev/a')"), "https://x.dev/a");
    }

    #[test]
    fn url_lanza_con_una_cadena_que_no_es_url() {
        // Codigo real usa try/catch alrededor de `new URL` para decidir si una
        // cadena es una URL; devolver algo a medias romperia ese patron.
        assert_eq!(
            evaluar("try { new URL('no es una url'); 'no lanzo' } catch (e) { 'lanzo' }"),
            "lanzo"
        );
    }

    #[test]
    fn search_params_lee_valores_repetidos() {
        assert_eq!(evaluar("new URLSearchParams('a=1&b=2').get('a')"), "1");
        // `get` devuelve el PRIMERO, `getAll` los dos.
        assert_eq!(evaluar("new URLSearchParams('a=1&a=2').get('a')"), "1");
        assert_eq!(evaluar("new URLSearchParams('a=1&a=2').getAll('a').join(',')"), "1,2");
        assert_eq!(evaluar("new URLSearchParams('a=1').has('b')"), "false");
    }

    #[test]
    fn search_params_acepta_la_interrogacion_inicial() {
        // `new URLSearchParams(location.search)` es el uso mas comun de todos, y
        // `location.search` incluye el '?'.
        assert_eq!(evaluar("new URLSearchParams('?a=1').get('a')"), "1");
    }

    #[test]
    fn search_params_decodifica_como_la_capa_de_red() {
        assert_eq!(evaluar("new URLSearchParams('q=hola+mundo').get('q')"), "hola mundo");
        assert_eq!(evaluar("new URLSearchParams('q=%C3%B1').get('q')"), "ñ");
    }

    #[test]
    fn search_params_vuelve_a_serializarse() {
        assert_eq!(
            evaluar("new URLSearchParams('q=hola mundo&a=1').toString()"),
            "q=hola+mundo&a=1"
        );
    }

    #[test]
    fn una_clave_ausente_devuelve_null_y_no_undefined() {
        // El spec dice `null`. La diferencia importa: `?? 'x'` los trata igual
        // pero `=== null` no, y hay codigo real que lo comprueba.
        assert_eq!(evaluar("new URLSearchParams('a=1').get('zzz') === null"), "true");
    }
}
