//! Soporte de módulos ES: `<script type="module">` y sus `import` (tarea C9
//! del `plan.md`, primera mitad).
//!
//! **Por qué esto es la tarea que desbloquea el bloque C.** Al verificar los
//! huecos contra el código (Fase 41) se vio que el techo del motor no eran las
//! APIs del DOM ausentes, como creía el README: era anterior. Todo bundle de
//! Vite, Next o Svelte se sirve como `<script type="module">`, y un
//! `<script type="module">` se ejecutaba aquí como un script clásico. Eso
//! significa que su primer `import` era un error de sintaxis y el bundle moría
//! antes de la primera línea útil, por muchas APIs que se añadieran después.
//!
//! **De dónde salen los módulos que se importan.** De lo que `core::server` ya
//! descargó ANTES de arrancar el JavaScript, no de la red en caliente. Es el
//! mismo patrón que el resto del motor (hojas de estilo, imágenes, scripts
//! clásicos): descubrir todas las URLs, descargarlas en paralelo con el
//! filtro de CSP aplicado, y solo entonces construir la página. Un cargador
//! que fuese a la red aquí dentro tendría que bloquear el hilo del intérprete
//! en mitad de la evaluación, y se saltaría el filtro de CSP que ya se aplicó
//! aguas arriba.
//!
//! Que eso baste para un bundle real no es casualidad: un `index.html` de Vite
//! declara sus fragmentos con `<link rel="modulepreload">` justo para que el
//! navegador los tenga antes de necesitarlos. `pipeline::find_module_preloads`
//! los recoge y `core::server` los descarga con los demás.
//!
//! **Lo que pasa cuando un import NO está precargado**: se lanza un error con
//! el especificador y la URL resuelta en el mensaje. No se devuelve un módulo
//! vacío. Un módulo vacío haría que el `import` funcionara y que el fallo
//! apareciera mucho más tarde, como un `undefined is not a function` en un
//! sitio sin relación con la causa.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use boa_engine::module::{ModuleLoader, Referrer};
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsString, Module, Source};
use boa_gc::GcRefCell;

/// Cargador que sirve módulos desde lo que ya se descargó para esta página.
pub struct PageModuleLoader {
    /// Código fuente por URL absoluta ya resuelta.
    fuentes: RefCell<HashMap<String, String>>,
    /// Módulos ya parseados, por URL absoluta.
    ///
    /// No es una optimización, es obligatorio: el spec exige que dos `import`
    /// del mismo especificador devuelvan EL MISMO módulo. Sin la caché, un
    /// módulo importado dos veces se evaluaría dos veces y su estado (un
    /// contador, una conexión, un registro de componentes) se duplicaría.
    /// `GcRefCell` porque `Module` lo gestiona el recolector de Boa.
    parseados: GcRefCell<HashMap<String, Module>>,
    /// URL de la página, base contra la que se resuelven los especificadores
    /// del módulo raíz.
    base: RefCell<String>,
}

impl PageModuleLoader {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            fuentes: RefCell::new(HashMap::new()),
            parseados: GcRefCell::new(HashMap::new()),
            base: RefCell::new(base.into()),
        }
    }

    /// Añade el código de un módulo ya descargado, indexado por su URL
    /// absoluta.
    pub fn insertar(&self, url: impl Into<String>, codigo: impl Into<String>) {
        self.fuentes.borrow_mut().insert(url.into(), codigo.into());
    }

    /// Cuántos módulos hay disponibles. Solo para tests y diagnóstico.
    pub fn len(&self) -> usize {
        self.fuentes.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.fuentes.borrow().is_empty()
    }

    /// Resuelve `especificador` contra `base` y devuelve la URL absoluta.
    ///
    /// Los especificadores "desnudos" (`import x from "react"`, sin `/`, `./`
    /// ni `../` delante) se rechazan a propósito: en la web NO tienen
    /// significado sin un *import map*, que este motor todavía no soporta.
    /// Resolverlos como si fueran rutas produciría una URL inventada, una
    /// descarga fallida y un error que apunta al sitio equivocado.
    fn resolver(&self, especificador: &str, base: &str) -> Result<String, String> {
        if !especificador.starts_with('/')
            && !especificador.starts_with("./")
            && !especificador.starts_with("../")
            && !especificador.contains("://")
        {
            return Err(format!(
                "especificador desnudo {especificador:?}: en la web solo son válidos con un \
                 <script type=\"importmap\">, que este motor todavía no soporta"
            ));
        }

        let base = url::Url::parse(base)
            .map_err(|e| format!("la URL base {base:?} no es válida ({e})"))?;
        base.join(especificador)
            .map(|u| u.to_string())
            .map_err(|e| format!("no se pudo resolver {especificador:?} contra {base} ({e})"))
    }

    /// Parsea (o recupera de caché) el módulo en `url`.
    fn modulo_de(&self, url: &str, context: &mut Context) -> JsResult<Module> {
        if let Some(cacheado) = self.parseados.borrow().get(url) {
            return Ok(cacheado.clone());
        }

        let codigo = self.fuentes.borrow().get(url).cloned();
        let Some(codigo) = codigo else {
            return Err(JsNativeError::typ()
                .with_message(format!(
                    "el módulo {url:?} no está entre los descargados para esta página. \
                     El motor no va a la red durante la evaluación: los módulos se \
                     descubren antes, por <script src> y <link rel=\"modulepreload\">"
                ))
                .into());
        };

        let modulo = Module::parse(Source::from_bytes(codigo.as_bytes()), None, context)?;
        self.parseados
            .borrow_mut()
            .insert(url.to_string(), modulo.clone());
        Ok(modulo)
    }

    /// Resuelve `src` contra la URL de la página. Devuelve `src` tal cual si
    /// no se puede resolver, que es mejor que perder la referencia entera.
    pub fn absoluta(&self, src: &str) -> String {
        let base = self.base.borrow().clone();
        url::Url::parse(&base)
            .and_then(|b| b.join(src))
            .map(|u| u.to_string())
            .unwrap_or_else(|_| src.to_string())
    }

    /// Registra un módulo ya parseado bajo `url`, para que un `import` a esa
    /// misma URL devuelva este y no vuelva a parsearlo.
    ///
    /// Lo usa el evaluador del módulo RAÍZ (el del `<script type="module">`),
    /// cuyo código no viene de un `import` sino del propio documento.
    pub fn registrar_raiz(&self, url: &str, modulo: Module) {
        self.parseados
            .borrow_mut()
            .insert(url.to_string(), modulo);
    }
}

impl ModuleLoader for PageModuleLoader {
    fn load_imported_module(
        &self,
        _referrer: Referrer,
        specifier: JsString,
        finish_load: Box<dyn FnOnce(JsResult<Module>, &mut Context)>,
        context: &mut Context,
    ) {
        let especificador = specifier.to_std_string_escaped();

        // Se resuelve contra la URL de la PÁGINA y no contra la del módulo que
        // importa. Es una simplificación declarada, no un descuido: para
        // conocer la URL del importador haría falta llevarla dentro de cada
        // `Module`, y Boa 0.19 no expone dónde guardarla (`host_defined` es
        // inmutable y `path` es un `Path` de disco, no una URL).
        //
        // Lo que esto cubre y lo que no: los bundlers emiten especificadores
        // ABSOLUTOS de ruta (`/assets/index-abc.js`), que se resuelven igual
        // contra la página que contra el importador, así que el caso que
        // motiva toda esta fase funciona. Lo que falla es un `./vecino.js`
        // entre dos módulos que NO estén en el mismo directorio que el
        // documento. Anotado en `huecos_sin_resolver.md`.
        let base = self.base.borrow().clone();
        let resultado = match self.resolver(&especificador, &base) {
            Ok(url) => self.modulo_de(&url, context),
            Err(motivo) => Err(JsNativeError::typ()
                .with_message(format!("no se pudo importar {especificador:?}: {motivo}"))
                .into()),
        };

        finish_load(resultado, context);
    }

    fn register_module(&self, specifier: JsString, module: Module) {
        self.parseados
            .borrow_mut()
            .insert(specifier.to_std_string_escaped(), module);
    }
}

/// Resultado de evaluar un módulo raíz.
pub enum ResultadoModulo {
    Ok,
    Error(String),
}

/// Evalúa `codigo` como módulo ES en `context`, usando `loader` para sus
/// `import`.
///
/// Devuelve el error como texto en vez de propagarlo porque quien llama
/// (`core::scripting::run_scripts`) trata cada `<script>` por separado: que uno
/// falle no debe impedir que corran los siguientes, igual que en un navegador
/// real.
pub fn evaluar_modulo(
    codigo: &str,
    url: &str,
    loader: &Rc<PageModuleLoader>,
    context: &mut Context,
) -> ResultadoModulo {
    let modulo = match Module::parse(Source::from_bytes(codigo.as_bytes()), None, context) {
        Ok(m) => m,
        Err(e) => return ResultadoModulo::Error(format!("error de sintaxis en el módulo: {e}")),
    };

    // Se registra ANTES de evaluar para que un import circular que vuelva a
    // este mismo módulo encuentre el que ya está en curso en vez de parsear
    // una copia nueva, que es lo que el spec exige.
    loader.registrar_raiz(url, modulo.clone());

    // `load_link_evaluate` hace las tres fases del spec (cargar el grafo,
    // enlazar los bindings, evaluar) y devuelve una promesa. Hay que correr
    // los trabajos pendientes para que esa promesa se resuelva: sin esto la
    // promesa queda pendiente para siempre y el módulo no se ejecuta, en
    // silencio.
    let promesa = modulo.load_link_evaluate(context);
    context.run_jobs();

    match promesa.state() {
        boa_engine::builtins::promise::PromiseState::Fulfilled(_) => ResultadoModulo::Ok,
        boa_engine::builtins::promise::PromiseState::Rejected(motivo) => {
            ResultadoModulo::Error(JsError::from_opaque(motivo).to_string())
        }
        // Pendiente tras drenar los trabajos significa que algo espera a algo
        // que nunca llegará (un `await` de una promesa que nadie resuelve).
        // Decirlo es mejor que devolver `Ok` y dejar la página a medias sin
        // ninguna pista.
        boa_engine::builtins::promise::PromiseState::Pending => ResultadoModulo::Error(
            "el módulo quedó pendiente tras agotar los trabajos: algo espera una promesa \
             que nadie resuelve"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::{js_string, JsValue};

    /// Construye un contexto con el cargador puesto, como hace `JsRuntime`.
    fn contexto_con(loader: Rc<PageModuleLoader>) -> Context {
        Context::builder()
            .module_loader(loader)
            .build()
            .expect("no se pudo construir el contexto")
    }

    /// Lee una global del contexto como texto, para comprobar efectos de un
    /// módulo (que no devuelve valor como un script clásico).
    fn leer_global(context: &mut Context, nombre: &str) -> String {
        context
            .global_object()
            .get(js_string!(nombre.to_string()), context)
            .unwrap_or(JsValue::undefined())
            .to_string(context)
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_else(|_| "<error>".to_string())
    }

    #[test]
    fn un_modulo_sin_imports_se_evalua() {
        let loader = Rc::new(PageModuleLoader::new("https://x.dev/index.html"));
        let mut ctx = contexto_con(loader.clone());

        let r = evaluar_modulo(
            "globalThis.resultado = 40 + 2;",
            "https://x.dev/app.js",
            &loader,
            &mut ctx,
        );
        assert!(matches!(r, ResultadoModulo::Ok));
        assert_eq!(leer_global(&mut ctx, "resultado"), "42");
    }

    #[test]
    fn un_modulo_importa_a_otro_ya_descargado() {
        let loader = Rc::new(PageModuleLoader::new("https://x.dev/index.html"));
        loader.insertar("https://x.dev/suma.js", "export function suma(a, b) { return a + b; }");
        let mut ctx = contexto_con(loader.clone());

        let r = evaluar_modulo(
            "import { suma } from '/suma.js'; globalThis.resultado = suma(2, 3);",
            "https://x.dev/app.js",
            &loader,
            &mut ctx,
        );
        assert!(matches!(r, ResultadoModulo::Ok), "el import deberia resolverse");
        assert_eq!(leer_global(&mut ctx, "resultado"), "5");
    }

    #[test]
    fn un_modulo_importado_dos_veces_solo_se_evalua_una() {
        // El spec lo exige y no es un detalle: si se evaluara dos veces, el
        // estado del modulo (un contador, un registro de componentes) se
        // duplicaria y los sintomas aparecerian muy lejos de la causa.
        let loader = Rc::new(PageModuleLoader::new("https://x.dev/index.html"));
        loader.insertar(
            "https://x.dev/contador.js",
            "globalThis.veces = (globalThis.veces || 0) + 1; export const x = 1;",
        );
        let mut ctx = contexto_con(loader.clone());

        let r = evaluar_modulo(
            "import { x } from '/contador.js'; import { x as y } from '/contador.js'; \
             globalThis.suma = x + y;",
            "https://x.dev/app.js",
            &loader,
            &mut ctx,
        );
        assert!(matches!(r, ResultadoModulo::Ok));
        assert_eq!(leer_global(&mut ctx, "veces"), "1", "se evaluo mas de una vez");
        assert_eq!(leer_global(&mut ctx, "suma"), "2");
    }

    #[test]
    fn un_import_que_no_esta_descargado_falla_con_un_mensaje_util() {
        // Devolver un modulo vacio haria que el `import` "funcionara" y que el
        // fallo apareciera mucho despues, como un `undefined is not a
        // function` sin relacion con la causa.
        let loader = Rc::new(PageModuleLoader::new("https://x.dev/index.html"));
        let mut ctx = contexto_con(loader.clone());

        let r = evaluar_modulo(
            "import { algo } from '/no-descargado.js'; globalThis.r = algo;",
            "https://x.dev/app.js",
            &loader,
            &mut ctx,
        );
        match r {
            ResultadoModulo::Error(e) => {
                assert!(
                    e.contains("no-descargado.js"),
                    "el error debe nombrar el modulo que falta: {e}"
                );
            }
            ResultadoModulo::Ok => panic!("deberia haber fallado"),
        }
    }

    #[test]
    fn un_especificador_desnudo_se_rechaza_diciendo_por_que() {
        // `import x from "react"` no tiene significado en la web sin un import
        // map. Resolverlo como ruta produciria una URL inventada y un error
        // que apunta al sitio equivocado.
        let loader = Rc::new(PageModuleLoader::new("https://x.dev/index.html"));
        let mut ctx = contexto_con(loader.clone());

        let r = evaluar_modulo("import x from 'react';", "https://x.dev/app.js", &loader, &mut ctx);
        match r {
            ResultadoModulo::Error(e) => assert!(
                e.contains("importmap") || e.contains("desnudo"),
                "el error debe explicar por que un especificador desnudo no vale: {e}"
            ),
            ResultadoModulo::Ok => panic!("deberia haber fallado"),
        }
    }

    #[test]
    fn la_sintaxis_de_modulo_ya_no_es_un_error() {
        // El nucleo de la fase: `export` a nivel superior es un error de
        // sintaxis en un script CLASICO. Que aqui parsee es justo la diferencia
        // entre ejecutar un bundle y no ejecutarlo.
        let loader = Rc::new(PageModuleLoader::new("https://x.dev/index.html"));
        let mut ctx = contexto_con(loader.clone());

        let r = evaluar_modulo("export const x = 1;", "https://x.dev/app.js", &loader, &mut ctx);
        assert!(matches!(r, ResultadoModulo::Ok));
    }

    #[test]
    fn un_error_en_tiempo_de_ejecucion_se_reporta_como_error() {
        let loader = Rc::new(PageModuleLoader::new("https://x.dev/index.html"));
        let mut ctx = contexto_con(loader.clone());

        let r = evaluar_modulo(
            "throw new Error('reventó');",
            "https://x.dev/app.js",
            &loader,
            &mut ctx,
        );
        match r {
            ResultadoModulo::Error(e) => assert!(e.contains("reventó"), "mensaje perdido: {e}"),
            ResultadoModulo::Ok => panic!("un módulo que lanza no puede reportarse como Ok"),
        }
    }

    #[test]
    fn resolver_maneja_rutas_absolutas_y_relativas() {
        let loader = PageModuleLoader::new("https://x.dev/a/index.html");
        assert_eq!(
            loader.resolver("/assets/i.js", "https://x.dev/a/index.html").unwrap(),
            "https://x.dev/assets/i.js"
        );
        assert_eq!(
            loader.resolver("./v.js", "https://x.dev/a/index.html").unwrap(),
            "https://x.dev/a/v.js"
        );
        assert_eq!(
            loader.resolver("../otro.js", "https://x.dev/a/index.html").unwrap(),
            "https://x.dev/otro.js"
        );
        assert!(loader.resolver("react", "https://x.dev/a/index.html").is_err());
    }
}
