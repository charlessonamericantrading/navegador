//! Páginas propias del navegador (inicio y error), en HTML/CSS que pinta el
//! motor. Los enlaces llevan `data-accion="ir:<url>"` o `data-accion="reintentar"`.

use crate::datos::Marcador;
use crate::lienzo::escapar;

/// Sitios de la página de inicio.
pub const SITIOS: [(&str, &str); 3] = [
    ("Wikipedia", "https://es.wikipedia.org"),
    ("Hacker News", "https://news.ycombinator.com"),
    ("DuckDuckGo", "https://lite.duckduckgo.com/lite/"),
];

/// La página de una pestaña nueva sin marcadores.
pub fn inicio() -> String {
    inicio_con_marcadores(&[])
}

/// La página de una pestaña nueva con accesos y marcadores guardados.
pub fn inicio_con_marcadores(marcadores: &[Marcador]) -> String {
    let enlaces = |items: &[(&str, &str)]| -> String {
        items
            .iter()
            .map(|(nombre, url)| format!(r#"<div class="sitio" data-accion="ir:{url}">{}</div>"#, escapar(nombre)))
            .collect()
    };
    let sitios = enlaces(&SITIOS);
    let seccion_marcadores = if marcadores.is_empty() {
        String::new()
    } else {
        let mut pills = String::new();
        for m in marcadores.iter().take(6) {
            let etiqueta = if m.titulo.trim().is_empty() { m.url.as_str() } else { m.titulo.as_str() };
            let corta: String = etiqueta.chars().take(18).collect();
            pills.push_str(&format!(
                r#"<div class="marcador" data-accion="ir:{}">{}</div>"#,
                escapar(&m.url),
                escapar(&corta)
            ));
        }
        format!(r#"<h2>Marcadores</h2><div id="marcadores">{pills}</div>"#)
    };
    format!(
        r#"<html><head><style>
body {{ margin: 0; background: #ffffff; font-family: sans-serif; color: #202124; }}
#centro {{ width: 560px; margin: 120px auto 0 auto; text-align: center; }}
h1 {{ font-size: 40px; font-weight: normal; margin: 0 0 12px 0; }}
h2 {{ font-size: 13px; font-weight: normal; color: #5f6368; margin: 36px 0 12px 0; }}
p {{ font-size: 16px; color: #5f6368; margin: 0 0 32px 0; }}
#sitios {{ display: flex; width: 504px; margin: 0 auto; }}
.sitio {{ width: 144px; height: 44px; line-height: 44px; margin: 0 12px; border-radius: 22px; background: #f1f3f4; font-size: 15px; text-align: center; }}
#marcadores {{ display: flex; width: 560px; margin: 0 auto; justify-content: center; flex-wrap: wrap; }}
.marcador {{ height: 32px; line-height: 32px; padding: 0 14px; margin: 4px 6px; border-radius: 16px; border: 1px solid #dadce0; background: #ffffff; color: #1a73e8; font-size: 13px; }}
</style></head><body><div id="centro">
<h1>Glowapp Browser</h1>
<p>Escribe una dirección o una búsqueda en la barra de arriba.</p>
<div id="sitios">{sitios}</div>
{seccion_marcadores}
</div></body></html>"#
    )
}

/// La página que se ve cuando una dirección no se puede abrir.
pub fn error(url: &str, mensaje: &str) -> String {
    format!(
        r#"<html><head><style>
body {{ margin: 0; background: #ffffff; font-family: sans-serif; color: #202124; }}
#centro {{ width: 600px; margin: 100px auto 0 auto; }}
h1 {{ font-size: 26px; font-weight: normal; margin: 0 0 16px 0; }}
p {{ font-size: 15px; color: #3c4043; margin: 0 0 12px 0; }}
.detalle {{ font-size: 13px; color: #80868b; margin-bottom: 28px; }}
#reintentar {{ width: 140px; height: 40px; line-height: 40px; border-radius: 20px; background: #1a73e8; color: #ffffff; font-size: 15px; text-align: center; }}
#botones {{ display: flex; }}
#diagnostico {{ width: 140px; height: 40px; line-height: 40px; margin-left: 12px; border-radius: 20px; background: #f1f3f4; color: #202124; font-size: 15px; text-align: center; }}
.ayuda {{ font-size: 13px; color: #80868b; margin-top: 20px; }}
</style></head><body><div id="centro">
<h1>{titulo}</h1>
<p>{url}</p>
<p class="detalle">{detalle}</p>
<div id="botones"><div id="reintentar" data-accion="reintentar">Reintentar</div><div id="diagnostico" data-accion="diagnostico">Diagnóstico</div></div>
<p class="ayuda">Si pasa con todas las webs, pulsa «Diagnóstico»: busca la causa y deja un informe que puedes enviar.</p>
</div></body></html>"#,
        titulo = escapar(explicar(mensaje)),
        url = escapar(url),
        detalle = escapar(mensaje),
    )
}

/// Una frase clara para el error del motor.
pub fn explicar(mensaje: &str) -> &'static str {
    let m = mensaje.to_lowercase();
    if m.contains("esquema_no_soportado") {
        "Este navegador solo abre direcciones http y https (los archivos del equipo, file://, todavía no)"
    } else if m.contains("timed out") || m.contains("timeout") || m.contains("tiempo") {
        "La página tardó demasiado en responder"
    } else if m.contains("dns") || m.contains("resolve") || m.contains("lookup") || m.contains("no such host") {
        "No se encuentra el sitio"
    } else if m.contains("refused") || m.contains("rechaz") {
        "El sitio rechazó la conexión"
    } else if m.contains("notvalidyet") || m.contains("not valid yet") {
        "La fecha del equipo no es correcta: el certificado del sitio aún no es válido"
    } else if m.contains("expired") || m.contains("caducad") || m.contains("expirad") {
        "El certificado del sitio ha caducado (si pasa en todas las webs, revisa la fecha y la hora del equipo)"
    } else if m.contains("unknownissuer") || m.contains("unknown issuer") {
        "El certificado del sitio no es de confianza en este equipo (a veces lo causa el antivirus o una red de empresa)"
    } else if m.contains("certificate") || m.contains("tls") || m.contains("certificado") || m.contains("invalid peer") || m.contains("webpki") {
        "La conexión segura con el sitio falló"
    } else if m.contains("url") && (m.contains("invalid") || m.contains("válida") || m.contains("valida")) {
        "La dirección no es válida"
    } else {
        "No se pudo abrir la página"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_pagina_de_inicio_lleva_el_nombre_de_la_store() {
        let html = inicio();
        assert!(html.contains("<h1>Glowapp Browser</h1>"), "{html}");
        assert!(!html.contains("<h1>Navegador</h1>"));
    }

    #[test]
    fn el_error_se_explica_claro() {
        assert_eq!(explicar("network_error: connection refused (os error 111)"), "El sitio rechazó la conexión");
        assert_eq!(explicar("network_error: operation timed out"), "La página tardó demasiado en responder");
        assert_eq!(
            explicar("network_error: HTTP request failed: client error (Connect): invalid peer certificate: Expired"),
            "El certificado del sitio ha caducado (si pasa en todas las webs, revisa la fecha y la hora del equipo)"
        );
        assert_eq!(explicar("algo raro"), "No se pudo abrir la página");
    }

    #[test]
    fn los_errores_de_certificado_se_explican_con_su_causa() {
        let desconocido = "HTTP request failed: client error (Connect): invalid peer certificate: UnknownIssuer";
        assert!(explicar(desconocido).contains("antivirus"), "{}", explicar(desconocido));
        assert!(explicar("invalid peer certificate: NotValidYet").contains("fecha del equipo"));
        assert!(explicar("invalid peer certificate: Expired").contains("caducado"));
        assert!(explicar("received fatal alert: HandshakeFailure (tls)").contains("conexión segura"));
    }

    #[test]
    fn un_esquema_no_soportado_se_explica_sin_culpar_a_la_red() {
        let m = "esquema_no_soportado: solo se admiten peticiones http y https (esquema: file)";
        assert!(explicar(m).contains("http y https"), "{}", explicar(m));
    }

    #[test]
    fn la_pagina_de_error_ofrece_el_diagnostico() {
        let html = error("https://x.com/", "fallo");
        assert!(html.contains(r#"data-accion="diagnostico""#));
        assert!(html.contains(r#"data-accion="reintentar""#));
    }

    #[test]
    fn el_error_no_mete_html_ajeno() {
        let html = error("https://x.com/<script>", "fallo <b>");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;b&gt;"));
    }
}
