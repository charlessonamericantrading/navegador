//! Content Security Policy (CSP) - Fase 21.
//!
//! CSP es la defensa principal contra XSS: deja que un servidor declare
//! de DONDE puede venir el codigo y los recursos de su pagina, de modo que
//! un script inyectado por un atacante no se ejecute aunque llegue a
//! colarse en el HTML.
//!
//! ## Que se aplica de verdad y donde
//!
//! Este motor tiene los ganchos exactos que CSP necesita, y por eso se
//! puede aplicar de punta a punta:
//! - `script-src`: `core::scripting` decide si ejecutar cada `<script>`
//!   (inline y externo).
//! - `style-src`: `core::server` decide si aplicar cada `<style>` y cada
//!   `<link rel=stylesheet>`.
//! - `img-src`: `core::server` decide si descargar cada `<img>`.
//! - `connect-src`: `engine-js` decide si dejar salir cada `fetch`/XHR.
//! - `default-src`: el respaldo de todas las anteriores.
//!
//! ## La regla que mas se malinterpreta
//!
//! **CSP solo restringe lo que menciona.** Si no hay `script-src` NI
//! `default-src`, los scripts se permiten - una politica que solo dice
//! `img-src 'self'` no bloquea JavaScript. Esto es asi en el spec y es lo
//! que hace que añadir CSP a un sitio existente no lo rompa entero.
//!
//! ## Simplificaciones declaradas
//!
//! - **Sin nonces ni hashes** (`'nonce-...'`, `'sha256-...'`): son la forma
//!   moderna y recomendada de permitir scripts inline concretos. Se
//!   PARSEAN (para no tratarlos como host) pero no habilitan nada, asi que
//!   una politica que dependa solo de un nonce bloqueara sus scripts. Es
//!   el lado seguro del error: bloquear de mas, nunca de menos.
//! - Sin `report-uri`/`report-to` (no se envian informes de violacion) ni
//!   `Content-Security-Policy-Report-Only` - una politica de solo-informe
//!   se ignora entera, que es exactamente lo que debe hacer: no bloquea
//!   nada por definicion.
//! - Sin `frame-ancestors`/`form-action`/`base-uri`: los dos primeros
//!   exigen `<iframe>` y navegacion de formulario con origen, el tercero
//!   `<base>`; ninguno existe en este motor todavia.
//! - Coincidencia de host simple: `*.ejemplo.test`, host exacto y `*`. Sin
//!   comparar puertos ni rutas dentro de la fuente.

use std::collections::HashMap;
use url::Url;

/// Una fuente permitida dentro de una directiva.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `'none'` - no permite NADA, ni siquiera el propio origen.
    None,
    /// `'self'` - el mismo origen que la pagina.
    SelfOrigin,
    /// `'unsafe-inline'` - permite `<script>`/`<style>` en linea.
    UnsafeInline,
    /// `*` - cualquier origen.
    Wildcard,
    /// `https:` - cualquier host, pero solo por ese esquema.
    Scheme(String),
    /// `ejemplo.test` o `*.ejemplo.test`.
    Host(String),
    /// `'nonce-...'`/`'sha256-...'` y cualquier otra palabra clave que este
    /// motor no evalua. Se guardan para NO confundirlas con un host (un
    /// `'nonce-abc'` tratado como host permitiria un dominio llamado
    /// "'nonce-abc'", que no existe, pero ensuciaria el razonamiento).
    Unsupported(String),
}

impl Source {
    fn parse(token: &str) -> Self {
        let token = token.trim();
        let lower = token.to_ascii_lowercase();
        if lower.starts_with('\'') {
            return match lower.trim_matches('\'') {
                "none" => Self::None,
                "self" => Self::SelfOrigin,
                "unsafe-inline" => Self::UnsafeInline,
                other => Self::Unsupported(other.to_string()),
            };
        }
        if lower == "*" {
            return Self::Wildcard;
        }
        // `https:` (con dos puntos al final y sin barras) es un esquema;
        // `https://ejemplo.test` es un host con esquema, que aqui se
        // reduce al host - no se comparan puertos ni rutas.
        if lower.ends_with(':') && !lower.contains("//") {
            return Self::Scheme(lower.trim_end_matches(':').to_string());
        }
        // `rsplit("//").next()` y no `split(..).next_back()`: `Split` con
        // separador de varios caracteres no es `DoubleEndedIterator`.
        let host = lower
            .rsplit("//")
            .next()
            .unwrap_or(&lower)
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        Self::Host(host)
    }

    fn matches_url(&self, url: &Url, page_origin: &str) -> bool {
        match self {
            Self::None | Self::UnsafeInline | Self::Unsupported(_) => false,
            Self::Wildcard => true,
            Self::SelfOrigin => crate::storage::origin_of(url) == page_origin,
            Self::Scheme(scheme) => url.scheme() == scheme,
            Self::Host(pattern) => {
                let Some(host) = url.host_str().map(|h| h.to_ascii_lowercase()) else { return false };
                // El host de la fuente puede traer puerto (`ejemplo.test:8080`);
                // se compara solo la parte de host, coherente con la
                // simplificacion declarada arriba.
                let pattern_host = pattern.split(':').next().unwrap_or(pattern);
                match pattern_host.strip_prefix("*.") {
                    // `*.ejemplo.test` cubre subdominios pero NO el dominio
                    // desnudo, igual que el spec.
                    Some(suffix) => host.len() > suffix.len() && host.ends_with(suffix) && host.as_bytes()[host.len() - suffix.len() - 1] == b'.',
                    None => host == pattern_host,
                }
            }
        }
    }
}

/// Las directivas que este motor puede APLICAR de verdad. Una directiva
/// fuera de esta lista se parsea igual (para no perderla al reserializar)
/// pero no gobierna nada - ver el doc-comment del modulo.
pub const ENFORCED_DIRECTIVES: &[&str] = &["default-src", "script-src", "style-src", "img-src", "connect-src"];

#[derive(Debug, Clone, Default)]
pub struct ContentSecurityPolicy {
    directives: HashMap<String, Vec<Source>>,
}

impl ContentSecurityPolicy {
    /// Parsea el valor de una cabecera (o `<meta>`) `Content-Security-Policy`.
    ///
    /// Varias politicas activas a la vez se combinan de forma
    /// RESTRICTIVA en el spec (un recurso debe pasarlas todas); aqui se
    /// parsea una sola y quien llame decide - ver `ContentSecurityPolicy::
    /// merge`.
    pub fn parse(header: &str) -> Self {
        let mut directives = HashMap::new();
        for part in header.split(';') {
            let mut tokens = part.split_whitespace();
            let Some(name) = tokens.next() else { continue };
            let name = name.to_ascii_lowercase();
            let sources: Vec<Source> = tokens.map(Source::parse).collect();
            // Una directiva sin fuentes equivale a `'none'` (no permite
            // nada), que es como la escribe mucha gente: `script-src;`.
            let sources = if sources.is_empty() { vec![Source::None] } else { sources };
            directives.insert(name, sources);
        }
        Self { directives }
    }

    pub fn is_empty(&self) -> bool {
        self.directives.is_empty()
    }

    /// Combina dos politicas de forma RESTRICTIVA: un recurso tiene que
    /// pasar las dos. Se usa cuando la pagina trae CSP por cabecera Y por
    /// `<meta>` a la vez - el spec exige que ambas se cumplan, no que la
    /// segunda relaje la primera.
    pub fn merge(&mut self, other: &Self) {
        for (name, sources) in &other.directives {
            match self.directives.get_mut(name) {
                // Ya existia: se queda la INTERSECCION conceptual, que aqui
                // se implementa como "hay que pasar las dos" en
                // `allows_url`/`allows_inline` - guardar la union de
                // fuentes seria relajar, justo lo contrario.
                Some(existing) => existing.retain(|s| sources.contains(s)),
                None => {
                    self.directives.insert(name.clone(), sources.clone());
                }
            }
        }
    }

    /// Las fuentes que gobiernan `directive`, cayendo a `default-src` si no
    /// esta declarada. `None` = ninguna de las dos existe, asi que **la
    /// politica no dice nada de esto y se permite** (ver la regla que mas
    /// se malinterpreta, en el doc del modulo).
    fn sources_for(&self, directive: &str) -> Option<&Vec<Source>> {
        self.directives.get(directive).or_else(|| self.directives.get("default-src"))
    }

    /// Si un recurso de esta URL se puede cargar bajo `directive`.
    pub fn allows_url(&self, directive: &str, url: &Url, page_origin: &str) -> bool {
        let Some(sources) = self.sources_for(directive) else { return true };
        if sources.iter().any(|s| *s == Source::None) {
            return false;
        }
        sources.iter().any(|s| s.matches_url(url, page_origin))
    }

    /// Si el contenido EN LINEA (`<script>...</script>`, `<style>...`) se
    /// puede ejecutar bajo `directive`. Exige `'unsafe-inline'` explicito:
    /// nonces y hashes no estan soportados (ver el doc del modulo), asi
    /// que una politica moderna basada en nonce bloqueara - el lado seguro
    /// del error.
    pub fn allows_inline(&self, directive: &str) -> bool {
        let Some(sources) = self.sources_for(directive) else { return true };
        sources.iter().any(|s| *s == Source::UnsafeInline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("URL de prueba valida")
    }

    const ORIGIN: &str = "https://web.test";

    /// La regla que mas se malinterpreta: CSP SOLO restringe lo que
    /// menciona. Sin `script-src` ni `default-src`, los scripts pasan.
    #[test]
    fn a_policy_only_restricts_what_it_mentions() {
        let csp = ContentSecurityPolicy::parse("img-src 'self'");
        assert!(csp.allows_url("script-src", &url("https://cdn.test/x.js"), ORIGIN));
        assert!(csp.allows_inline("script-src"));
        // Pero lo que SI menciona, lo restringe.
        assert!(!csp.allows_url("img-src", &url("https://cdn.test/x.png"), ORIGIN));
    }

    #[test]
    fn default_src_is_the_fallback_for_an_undeclared_directive() {
        let csp = ContentSecurityPolicy::parse("default-src 'self'");
        assert!(csp.allows_url("script-src", &url("https://web.test/app.js"), ORIGIN));
        assert!(!csp.allows_url("script-src", &url("https://cdn.test/app.js"), ORIGIN));
    }

    /// Una directiva especifica GANA sobre `default-src`, no se suma a
    /// ella.
    #[test]
    fn a_specific_directive_overrides_default_src_instead_of_adding_to_it() {
        let csp = ContentSecurityPolicy::parse("default-src 'none'; script-src https://cdn.test");
        assert!(csp.allows_url("script-src", &url("https://cdn.test/x.js"), ORIGIN));
        assert!(!csp.allows_url("img-src", &url("https://cdn.test/x.png"), ORIGIN), "img-src sigue cayendo a default-src 'none'");
    }

    #[test]
    fn self_matches_only_the_page_origin() {
        let csp = ContentSecurityPolicy::parse("script-src 'self'");
        assert!(csp.allows_url("script-src", &url("https://web.test/a.js"), ORIGIN));
        assert!(!csp.allows_url("script-src", &url("https://otro.test/a.js"), ORIGIN));
        assert!(!csp.allows_url("script-src", &url("http://web.test/a.js"), ORIGIN), "otro esquema es otro origen");
    }

    #[test]
    fn none_blocks_everything_including_the_page_own_origin() {
        let csp = ContentSecurityPolicy::parse("script-src 'none'");
        assert!(!csp.allows_url("script-src", &url("https://web.test/a.js"), ORIGIN));
        assert!(!csp.allows_inline("script-src"));
    }

    #[test]
    fn a_directive_without_sources_behaves_as_none() {
        let csp = ContentSecurityPolicy::parse("script-src;");
        assert!(!csp.allows_url("script-src", &url("https://web.test/a.js"), ORIGIN));
    }

    #[test]
    fn inline_needs_unsafe_inline_explicitly() {
        assert!(!ContentSecurityPolicy::parse("script-src 'self'").allows_inline("script-src"));
        assert!(ContentSecurityPolicy::parse("script-src 'self' 'unsafe-inline'").allows_inline("script-src"));
    }

    /// Nonces y hashes se parsean pero no habilitan nada - el lado seguro
    /// del error, declarado en el modulo.
    #[test]
    fn a_nonce_does_not_enable_inline_because_nonces_are_not_supported() {
        let csp = ContentSecurityPolicy::parse("script-src 'nonce-abc123'");
        assert!(!csp.allows_inline("script-src"), "sin soporte de nonce, bloquear es el lado seguro");
        // Y sobre todo: NO debe tratarse como un host llamado 'nonce-abc123'.
        assert!(!csp.allows_url("script-src", &url("https://nonce-abc123/x.js"), ORIGIN));
    }

    #[test]
    fn a_wildcard_allows_any_origin() {
        let csp = ContentSecurityPolicy::parse("img-src *");
        assert!(csp.allows_url("img-src", &url("https://cualquiera.test/x.png"), ORIGIN));
    }

    #[test]
    fn a_scheme_source_allows_any_host_over_that_scheme_only() {
        let csp = ContentSecurityPolicy::parse("img-src https:");
        assert!(csp.allows_url("img-src", &url("https://cualquiera.test/x.png"), ORIGIN));
        assert!(!csp.allows_url("img-src", &url("http://cualquiera.test/x.png"), ORIGIN));
    }

    #[test]
    fn a_host_source_matches_that_exact_host() {
        let csp = ContentSecurityPolicy::parse("script-src cdn.test");
        assert!(csp.allows_url("script-src", &url("https://cdn.test/x.js"), ORIGIN));
        assert!(!csp.allows_url("script-src", &url("https://malcdn.test/x.js"), ORIGIN));
    }

    /// `*.ejemplo.test` cubre subdominios pero NO el dominio desnudo, y
    /// exige un separador de etiqueta real - el mismo criterio que las
    /// cookies, y por la misma razon: sin el, `malcdn.test` colaria.
    #[test]
    fn a_wildcard_host_covers_subdomains_only_at_a_label_boundary() {
        let csp = ContentSecurityPolicy::parse("script-src *.cdn.test");
        assert!(csp.allows_url("script-src", &url("https://a.cdn.test/x.js"), ORIGIN));
        assert!(!csp.allows_url("script-src", &url("https://cdn.test/x.js"), ORIGIN), "el comodin no cubre el dominio desnudo");
        assert!(!csp.allows_url("script-src", &url("https://malcdn.test/x.js"), ORIGIN));
    }

    #[test]
    fn a_source_with_a_scheme_prefix_is_reduced_to_its_host() {
        let csp = ContentSecurityPolicy::parse("script-src https://cdn.test/ruta/");
        assert!(csp.allows_url("script-src", &url("https://cdn.test/otra.js"), ORIGIN), "sin comparar rutas, simplificacion declarada");
    }

    #[test]
    fn several_directives_in_one_header_are_all_parsed() {
        let csp = ContentSecurityPolicy::parse("default-src 'self'; script-src 'self' cdn.test; img-src *");
        assert!(csp.allows_url("script-src", &url("https://cdn.test/x.js"), ORIGIN));
        assert!(csp.allows_url("img-src", &url("http://cualquiera.test/x.png"), ORIGIN));
        assert!(!csp.allows_url("connect-src", &url("https://api.test/x"), ORIGIN), "connect-src cae a default-src 'self'");
    }

    /// Dos politicas activas se combinan de forma RESTRICTIVA: hay que
    /// pasar las dos. Combinar relajando seria un agujero - un atacante
    /// que pudiera inyectar un `<meta>` desactivaria la CSP del servidor.
    #[test]
    fn merging_two_policies_is_restrictive_not_permissive() {
        let mut cabecera = ContentSecurityPolicy::parse("script-src 'self' cdn.test");
        let meta = ContentSecurityPolicy::parse("script-src 'self'");
        cabecera.merge(&meta);
        assert!(cabecera.allows_url("script-src", &url("https://web.test/a.js"), ORIGIN));
        assert!(
            !cabecera.allows_url("script-src", &url("https://cdn.test/a.js"), ORIGIN),
            "lo que solo permitia una de las dos deberia quedar bloqueado"
        );
    }

    #[test]
    fn an_empty_policy_allows_everything() {
        let csp = ContentSecurityPolicy::parse("");
        assert!(csp.is_empty());
        assert!(csp.allows_url("script-src", &url("https://cualquiera.test/x.js"), ORIGIN));
        assert!(csp.allows_inline("script-src"));
    }

    #[test]
    fn directive_names_are_case_insensitive() {
        let csp = ContentSecurityPolicy::parse("SCRIPT-SRC 'NONE'");
        assert!(!csp.allows_url("script-src", &url("https://web.test/a.js"), ORIGIN));
    }
}
