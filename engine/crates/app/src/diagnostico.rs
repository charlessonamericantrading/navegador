//! `navegador --diagnostico`: averigua por qué no abre páginas y lo deja escrito.
//!
//! Hace dos tipos de prueba y las compara:
//! 1. **Directas**: desde la propia app, sin motor ni aislamiento: DNS, conexión
//!    TCP y una petición HTTP/HTTPS con la misma pila de red del motor.
//! 2. **Con el motor completo**: el mismo camino que la ventana (proceso del
//!    broker + motor aislado en Windows), cargando la página.
//!
//! Si fallan las directas, el problema es de la red, del certificado o de la
//! hora del equipo. Si las directas van bien y las del motor no, el problema es
//! del motor o de su aislamiento. El informe acaba con las pistas que salen de
//! esa comparación y con las últimas líneas del registro.

use crate::registro;
use crate::sin_ventana::Sesion;
use engine_net::{NetworkEngine, NetworkRequest};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Argumento con el que el ejecutable hace el diagnóstico.
pub const ARGUMENTO: &str = "--diagnostico";
/// Variable para cambiar las direcciones de prueba (separadas por comas). Las
/// pruebas automáticas la usan para no depender de internet.
pub const URLS_ENV: &str = "NAVEGADOR_DIAGNOSTICO_URLS";
/// Si vale `1`, no se abre el informe en el Bloc de notas al terminar.
pub const NO_ABRIR_ENV: &str = "NAVEGADOR_DIAGNOSTICO_NO_ABRIR";

const URLS_POR_DEFECTO: [&str; 4] = ["http://example.com/", "https://example.com/", "https://es.wikipedia.org/", "https://www.google.com/"];
const ESPERA_DNS_TCP: Duration = Duration::from_secs(6);
const ESPERA_HTTP: Duration = Duration::from_secs(15);
const ESPERA_MOTOR: Duration = Duration::from_secs(40);
/// A partir de cuántos segundos de diferencia con la hora del servidor se avisa.
const DESFASE_AVISO_S: i64 = 300;

/// Resultado de las pruebas directas contra una dirección.
#[derive(Debug, Clone)]
pub struct PruebaDirecta {
    pub url: String,
    pub dns: Result<Vec<String>, String>,
    /// `None` si no se probó porque el DNS falló.
    pub tcp: Option<Result<u128, String>>,
    /// Estado HTTP, milisegundos y diferencia (segundos) entre la hora del servidor y la del equipo.
    pub http: Result<(u16, u128, Option<i64>), String>,
}

/// Resultado de cargar una dirección con el motor completo.
#[derive(Debug, Clone)]
pub struct PruebaMotor {
    pub url: String,
    pub resultado: Result<String, String>,
}

/// Las direcciones que se prueban.
pub fn urls() -> Vec<String> {
    match std::env::var(URLS_ENV) {
        Ok(lista) if !lista.trim().is_empty() => lista.split(',').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect(),
        _ => URLS_POR_DEFECTO.iter().map(|u| (*u).to_string()).collect(),
    }
}

/// Hace el diagnóstico completo y devuelve el informe como texto.
pub fn ejecutar(ejecutable: &Path) -> String {
    let urls = urls();
    let directas: Vec<PruebaDirecta> = urls.iter().map(|u| probar_directo(u)).collect();
    // Con el motor completo solo se prueban las tres primeras: cada una arranca
    // el broker y el motor, y no hace falta repetir lo mismo con todas.
    let motor: Vec<PruebaMotor> = urls.iter().take(3).map(|u| probar_motor(ejecutable, u)).collect();
    informe(ejecutable, &directas, &motor)
}

/// Compone el texto del informe (separado para poder probarlo sin red).
pub fn informe(ejecutable: &Path, directas: &[PruebaDirecta], motor: &[PruebaMotor]) -> String {
    let ahora = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut t = String::new();
    t.push_str("DIAGNÓSTICO DEL NAVEGADOR\n=========================\n");
    t.push_str(&format!("Fecha (hora del equipo): {}\n", registro::fecha_utc(ahora)));
    t.push_str(&format!("Versión: {}\n", env!("CARGO_PKG_VERSION")));
    t.push_str(&format!("Sistema: {} ({})\n", std::env::consts::OS, std::env::consts::ARCH));
    t.push_str(&format!("Programa: {}\n", ejecutable.display()));
    match engine_core::process::profile_dir() {
        Some(dir) => t.push_str(&format!("Datos: {} ({})\n", dir.display(), if se_puede_escribir(&dir) { "se puede escribir" } else { "NO se puede escribir" })),
        None => t.push_str("Datos: no hay carpeta de datos\n"),
    }

    t.push_str("\n1) RED DIRECTA (sin motor ni aislamiento)\n");
    for p in directas {
        t.push_str(&format!("- {}\n", p.url));
        match &p.dns {
            Ok(ips) => t.push_str(&format!("    nombre → direcciones: {}\n", ips.join(", "))),
            Err(e) => t.push_str(&format!("    nombre → FALLA: {e}\n")),
        }
        match &p.tcp {
            Some(Ok(ms)) => t.push_str(&format!("    conexión: bien ({ms} ms)\n")),
            Some(Err(e)) => t.push_str(&format!("    conexión: FALLA: {e}\n")),
            None => t.push_str("    conexión: no se probó\n"),
        }
        match &p.http {
            Ok((estado, ms, desfase)) => {
                t.push_str(&format!("    página: bien (estado {estado}, {ms} ms)\n"));
                if let Some(d) = desfase {
                    t.push_str(&format!("    hora del servidor frente a la del equipo: {d:+} s\n"));
                }
            }
            Err(e) => t.push_str(&format!("    página: FALLA: {e}\n")),
        }
    }

    t.push_str("\n2) CON EL MOTOR COMPLETO (como la ventana: broker + motor)\n");
    for m in motor {
        match &m.resultado {
            Ok(r) => t.push_str(&format!("- {}: bien → {r}\n", m.url)),
            Err(e) => t.push_str(&format!("- {}: FALLA: {e}\n", m.url)),
        }
    }

    t.push_str("\n3) PISTAS\n");
    for pista in pistas(directas, motor) {
        t.push_str(&format!("- {pista}\n"));
    }

    t.push_str("\n4) ÚLTIMAS LÍNEAS DEL REGISTRO\n");
    match registro::ruta() {
        Some(ruta) => {
            let lineas = registro::cola(&ruta, 30);
            if lineas.is_empty() {
                t.push_str("(vacío)\n");
            }
            for l in lineas {
                t.push_str(&format!("{l}\n"));
            }
        }
        None => t.push_str("(no hay carpeta de datos)\n"),
    }
    anonimizar(&t, &carpeta_personal())
}

/// Lo que se deduce de comparar las pruebas, en frases cortas.
pub fn pistas(directas: &[PruebaDirecta], motor: &[PruebaMotor]) -> Vec<String> {
    let mut p = Vec::new();
    let errores: Vec<String> = directas
        .iter()
        .flat_map(|d| {
            let mut e = Vec::new();
            if let Err(x) = &d.dns {
                e.push(format!("dns:{x}"));
            }
            if let Some(Err(x)) = &d.tcp {
                e.push(format!("tcp:{x}"));
            }
            if let Err(x) = &d.http {
                e.push(format!("http:{x}"));
            }
            e
        })
        .collect();
    let normal = |s: &str| s.to_lowercase().replace([' ', '_', '-'], "");
    let hay = |clave: &str| errores.iter().any(|e| normal(e).contains(clave));

    if hay("unknownissuer") || (hay("invalidpeercertificate") && !hay("expired") && !hay("notvalidyet")) {
        p.push("El certificado de las webs seguras no es de confianza para el navegador. Suele pasar si el antivirus revisa las conexiones seguras (Avast, Kaspersky, ESET, Bitdefender...) o si estás en una red de empresa o colegio. Prueba a desactivar en el antivirus la opción de «analizar conexiones cifradas / HTTPS» y repite el diagnóstico.".to_string());
    }
    if hay("notvalidyet") || hay("expired") {
        p.push("Un certificado aparece como caducado o aún no válido. Revisa que la fecha, la hora y la zona horaria del equipo sean correctas.".to_string());
    }
    // Se mira la mediana de todos los servidores: uno solo con la hora rara (una caché
    // vieja, un servidor mal puesto) no significa que el reloj del equipo esté mal.
    let mut desfases: Vec<i64> = directas.iter().filter_map(|d| d.http.as_ref().ok().and_then(|(_, _, s)| *s)).collect();
    desfases.sort_unstable();
    let desfase_grande = desfases.get(desfases.len() / 2).is_some_and(|mediana| mediana.abs() > DESFASE_AVISO_S);
    if desfase_grande {
        p.push("La hora del equipo se desvía más de 5 minutos de la del servidor: corrígela (fecha y hora automáticas).".to_string());
    }
    if !directas.is_empty() && directas.iter().all(|d| d.dns.is_err()) {
        p.push("Ninguna dirección se resuelve: parece que no hay internet o que el DNS no responde. ¿Funcionan otras aplicaciones? Revisa la conexión, la VPN o el DNS.".to_string());
    } else if directas.iter().any(|d| d.dns.is_ok() && matches!(d.tcp, Some(Err(_)))) {
        p.push("El nombre se resuelve pero no se logra conectar: algo bloquea la conexión (cortafuegos, antivirus, proxy o red de empresa). Prueba a permitir navegador.exe en el cortafuegos.".to_string());
    }
    let http_ok = directas.iter().any(|d| d.url.starts_with("http://") && d.http.is_ok());
    let https_ok = directas.iter().any(|d| d.url.starts_with("https://") && d.http.is_ok());
    let https_falla = directas.iter().any(|d| d.url.starts_with("https://") && d.http.is_err());
    if http_ok && !https_ok && https_falla {
        p.push("Las páginas sin cifrar (http) funcionan y las seguras (https) no: el problema está en la conexión segura (certificados, antivirus o hora).".to_string());
    }
    let motor_ok = motor.iter().any(|m| m.resultado.is_ok());
    let motor_falla = !motor.is_empty() && motor.iter().all(|m| m.resultado.is_err());
    if (http_ok || https_ok) && motor_falla {
        p.push("La red directa funciona pero el motor completo no carga nada: el fallo está en el motor, en el broker o en el aislamiento de Windows. Envía este informe.".to_string());
    }
    if motor.iter().any(|m| matches!(&m.resultado, Err(e) if e.contains("no arrancó el motor"))) {
        p.push("El motor ni siquiera arranca. Mira el mensaje de arriba: suele ser un problema de permisos o de aislamiento de Windows. Envía este informe.".to_string());
    }
    if p.is_empty() {
        if http_ok || https_ok {
            if motor_ok {
                p.push("La red y el motor responden bien. Si una web concreta no se ve o se ve mal, es una limitación del motor con esa web, no un fallo de conexión (YouTube, Gmail y otras aplicaciones web grandes aún no funcionan).".to_string());
            } else {
                p.push("La red directa funciona. No hay una causa clara en el motor: envía este informe.".to_string());
            }
        } else {
            p.push("Fallan todas las pruebas sin una causa reconocida: envía este informe.".to_string());
        }
    }
    p
}

/// Prueba directa de una dirección: DNS, TCP y petición.
pub fn probar_directo(url: &str) -> PruebaDirecta {
    let Ok(parsed) = url::Url::parse(url) else {
        let e = "dirección no válida".to_string();
        return PruebaDirecta { url: url.to_string(), dns: Err(e.clone()), tcp: None, http: Err(e) };
    };
    let host = parsed.host_str().unwrap_or_default().to_string();
    let puerto = parsed.port_or_known_default().unwrap_or(80);

    let direcciones = resolver(&host, puerto);
    let dns = direcciones.as_ref().map(|d| d.iter().map(ToString::to_string).collect()).map_err(Clone::clone);
    let tcp = match &direcciones {
        Ok(lista) => {
            let mut ultimo = "sin direcciones".to_string();
            let mut resultado = None;
            for dir in lista.iter().take(3) {
                let t0 = Instant::now();
                match TcpStream::connect_timeout(dir, ESPERA_DNS_TCP) {
                    Ok(_) => {
                        resultado = Some(Ok(t0.elapsed().as_millis()));
                        break;
                    }
                    Err(e) => ultimo = format!("{dir}: {e}"),
                }
            }
            Some(resultado.unwrap_or(Err(ultimo)))
        }
        Err(_) => None,
    };
    let http = peticion(url);
    PruebaDirecta { url: url.to_string(), dns, tcp, http }
}

/// Resuelve el nombre con un tiempo máximo (el sistema operativo no lo ofrece, así que va en un hilo).
fn resolver(host: &str, puerto: u16) -> Result<Vec<std::net::SocketAddr>, String> {
    let (enviar, recibir) = std::sync::mpsc::channel();
    let nombre = host.to_string();
    std::thread::spawn(move || {
        let r = (nombre.as_str(), puerto).to_socket_addrs().map(|i| i.collect::<Vec<_>>()).map_err(|e| e.to_string());
        let _ = enviar.send(r);
    });
    match recibir.recv_timeout(ESPERA_DNS_TCP) {
        Ok(Ok(lista)) if lista.is_empty() => Err("sin direcciones".to_string()),
        Ok(r) => r,
        Err(_) => Err(format!("no contestó en {} s", ESPERA_DNS_TCP.as_secs())),
    }
}

fn peticion(url: &str) -> Result<(u16, u128, Option<i64>), String> {
    let pedido = NetworkRequest::new(url).map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let respuesta = runtime.block_on(async { NetworkEngine::new().with_request_timeout(ESPERA_HTTP).fetch(&pedido).await }).map_err(|e| e.to_string())?;
    let ahora = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let cabecera = |nombre: &str| respuesta.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(nombre)).map(|(_, v)| v.as_str());
    // Una respuesta que salió de una caché trae la hora en que se guardó: hay que
    // sumarle su edad (`Age`) para saber qué hora era de verdad en el servidor.
    let edad = cabecera("age").and_then(|a| a.trim().parse::<i64>().ok()).unwrap_or(0);
    let desfase = cabecera("date").and_then(unix_de_fecha_http).map(|servidor| servidor + edad - ahora);
    Ok((respuesta.status_code, t0.elapsed().as_millis(), desfase))
}

/// Carga una dirección con el mismo camino que la ventana (broker + motor).
pub fn probar_motor(ejecutable: &Path, url: &str) -> PruebaMotor {
    let resultado = match Sesion::nueva(ejecutable, 900, 600, Some(url.to_string())) {
        Err(e) => {
            registro::escribir(&format!("diagnóstico: el motor no arrancó: {e}"));
            Err(format!("no arrancó el motor: {e}"))
        }
        Ok(sesion) => {
            let mut sesion = sesion.con_registro();
            let terminado = sesion.esperar_reposo(ESPERA_MOTOR);
            let app = sesion.app();
            let r = if let Some((_, mensaje)) = app.error_visible() {
                Err(mensaje.clone())
            } else if !terminado {
                Err(format!("el motor no contestó en {} s", ESPERA_MOTOR.as_secs()))
            } else if app.url().is_empty() {
                Err("no hay página cargada".to_string())
            } else {
                Ok(format!("{} — «{}»", app.url(), app.titulo()))
            };
            sesion.cerrar();
            r
        }
    };
    PruebaMotor { url: url.to_string(), resultado }
}

/// `Thu, 26 Nov 2026 16:54:09 GMT` → segundos desde 1970.
pub fn unix_de_fecha_http(fecha: &str) -> Option<i64> {
    let partes: Vec<&str> = fecha.split_whitespace().collect();
    if partes.len() < 5 {
        return None;
    }
    let dia: u32 = partes[1].parse().ok()?;
    let mes = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|m| m.eq_ignore_ascii_case(partes[2]))? as u32 + 1;
    let anio: i64 = partes[3].parse().ok()?;
    let mut hms = partes[4].split(':').map(|n| n.parse::<i64>().ok());
    let (h, m, s) = (hms.next()??, hms.next()??, hms.next()??);
    Some(registro::dias_desde_civil(anio, mes, dia) * 86_400 + h * 3600 + m * 60 + s)
}

fn se_puede_escribir(dir: &Path) -> bool {
    let prueba = dir.join(".prueba-escritura");
    let _ = std::fs::create_dir_all(dir);
    let ok = std::fs::write(&prueba, b"x").is_ok();
    let _ = std::fs::remove_file(prueba);
    ok
}

/// La carpeta personal del usuario, para no enseñar su nombre en el informe.
fn carpeta_personal() -> String {
    std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default()
}

/// Cambia la carpeta personal por `%USUARIO%` (el informe se va a compartir).
pub fn anonimizar(texto: &str, carpeta_personal: &str) -> String {
    if carpeta_personal.len() < 4 {
        return texto.to_string();
    }
    texto.replace(carpeta_personal, "%USUARIO%")
}

/// Ejecuta el diagnóstico, lo guarda en `diagnostico.txt`, lo muestra y (en
/// Windows) lo abre en el Bloc de notas. Devuelve dónde quedó.
pub fn correr() -> Result<PathBuf, String> {
    let ejecutable = std::env::current_exe().map_err(|e| format!("no se encuentra el programa: {e}"))?;
    registro::escribir("diagnóstico: empieza");
    if std::env::var(NO_ABRIR_ENV).as_deref() != Ok("1") {
        crate::aviso::mostrar_sin_esperar("Navegador", "Ejecutando el diagnóstico. Tarda hasta un minuto; al terminar se abrirá un informe en el Bloc de notas.\n\nPuedes cerrar este aviso.", false);
    }
    let texto = ejecutar(&ejecutable);
    let dir = engine_core::process::profile_dir().unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&dir);
    let ruta = dir.join("diagnostico.txt");
    std::fs::write(&ruta, &texto).map_err(|e| format!("no se pudo guardar {}: {e}", ruta.display()))?;
    registro::escribir(&format!("diagnóstico: guardado en {}", ruta.display()));
    println!("{texto}\nInforme guardado en: {}", ruta.display());
    abrir(&ruta);
    Ok(ruta)
}

#[cfg(windows)]
fn abrir(ruta: &Path) {
    if std::env::var(NO_ABRIR_ENV).as_deref() != Ok("1") {
        let _ = std::process::Command::new("notepad.exe").arg(ruta).spawn();
    }
}

#[cfg(not(windows))]
fn abrir(_ruta: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn directa(url: &str, dns: Result<(), &str>, tcp: Option<Result<(), &str>>, http: Result<(u16, Option<i64>), &str>) -> PruebaDirecta {
        PruebaDirecta {
            url: url.to_string(),
            dns: dns.map(|()| vec!["1.2.3.4:443".to_string()]).map_err(str::to_string),
            tcp: tcp.map(|r| r.map(|()| 10).map_err(str::to_string)),
            http: http.map(|(e, d)| (e, 50, d)).map_err(str::to_string),
        }
    }
    fn motor(url: &str, r: Result<&str, &str>) -> PruebaMotor {
        PruebaMotor { url: url.to_string(), resultado: r.map(str::to_string).map_err(str::to_string) }
    }

    /// Servidor HTTP mínimo que contesta 200 con su hora.
    fn servidor() -> u16 {
        let escucha = TcpListener::bind("127.0.0.1:0").unwrap();
        let puerto = escucha.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in escucha.incoming().flatten() {
                let mut c = c;
                let mut buf = [0u8; 1024];
                let _ = c.read(&mut buf);
                let cuerpo = "<html><head><title>Hola</title></head><body>hola</body></html>";
                let r = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{cuerpo}", cuerpo.len());
                let _ = c.write_all(r.as_bytes());
            }
        });
        puerto
    }

    #[test]
    fn lee_la_fecha_http() {
        assert_eq!(unix_de_fecha_http("Thu, 26 Nov 2026 16:54:09 GMT"), Some(1_795_712_049));
        assert_eq!(unix_de_fecha_http("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(unix_de_fecha_http("basura"), None);
        assert_eq!(unix_de_fecha_http("Thu, 99 Xxx 2026 16:54:09 GMT"), None);
    }

    #[test]
    fn pista_de_certificado_no_confiable() {
        let d = [directa("https://example.com/", Ok(()), Some(Ok(())), Err("HTTP request failed: client error (Connect): invalid peer certificate: UnknownIssuer"))];
        let p = pistas(&d, &[]).join(" ");
        assert!(p.contains("antivirus"), "{p}");
    }

    #[test]
    fn pista_de_hora_incorrecta() {
        let d = [directa("https://example.com/", Ok(()), Some(Ok(())), Err("client error (Connect): invalid peer certificate: NotValidYet"))];
        assert!(pistas(&d, &[]).join(" ").contains("fecha, la hora"));
        let d = [directa("http://example.com/", Ok(()), Some(Ok(())), Ok((200, Some(4000))))];
        assert!(pistas(&d, &[]).join(" ").contains("5 minutos"));
        let d = [directa("http://example.com/", Ok(()), Some(Ok(())), Ok((200, Some(-20))))];
        assert!(!pistas(&d, &[]).join(" ").contains("5 minutos"));
        // Todos los servidores coinciden en que el reloj va adelantado: aviso.
        let todos = [
            directa("https://a.com/", Ok(()), Some(Ok(())), Ok((200, Some(-4000)))),
            directa("https://b.com/", Ok(()), Some(Ok(())), Ok((200, Some(-3990)))),
            directa("https://c.com/", Ok(()), Some(Ok(())), Ok((200, Some(-4010)))),
        ];
        assert!(pistas(&todos, &[]).join(" ").contains("5 minutos"));
    }

    #[test]
    fn un_servidor_con_la_hora_rara_no_acusa_al_reloj_del_equipo() {
        // Caso real: una respuesta de Wikipedia salida de una caché, 10 horas vieja,
        // frente a otros dos servidores que coinciden con el equipo.
        let d = [
            directa("https://example.com/", Ok(()), Some(Ok(())), Ok((200, Some(0)))),
            directa("https://es.wikipedia.org/", Ok(()), Some(Ok(())), Ok((200, Some(-35_096)))),
            directa("https://www.google.com/", Ok(()), Some(Ok(())), Ok((200, Some(1)))),
        ];
        assert!(!pistas(&d, &[]).join(" ").contains("5 minutos"), "{:?}", pistas(&d, &[]));
    }

    #[test]
    fn pista_de_sin_internet_y_de_bloqueo() {
        let sin = [directa("http://a.com/", Err("fallo"), None, Err("x")), directa("https://a.com/", Err("fallo"), None, Err("x"))];
        assert!(pistas(&sin, &[]).join(" ").contains("no hay internet"));
        let bloqueo = [directa("http://a.com/", Ok(()), Some(Err("denegado")), Err("x"))];
        assert!(pistas(&bloqueo, &[]).join(" ").contains("cortafuegos"));
    }

    #[test]
    fn pista_de_solo_https_roto() {
        let d = [directa("http://a.com/", Ok(()), Some(Ok(())), Ok((200, None))), directa("https://a.com/", Ok(()), Some(Ok(())), Err("otro fallo"))];
        assert!(pistas(&d, &[]).join(" ").contains("conexión segura"));
    }

    #[test]
    fn pista_de_motor_roto_con_la_red_bien() {
        let d = [directa("https://a.com/", Ok(()), Some(Ok(())), Ok((200, None)))];
        let m = [motor("https://a.com/", Err("no arrancó el motor: acceso denegado"))];
        let p = pistas(&d, &m).join(" ");
        assert!(p.contains("el fallo está en el motor"), "{p}");
        assert!(p.contains("ni siquiera arranca"), "{p}");
    }

    #[test]
    fn pista_cuando_todo_va_bien() {
        let d = [directa("https://a.com/", Ok(()), Some(Ok(())), Ok((200, Some(2))))];
        let m = [motor("https://a.com/", Ok("https://a.com/ — «A»"))];
        let p = pistas(&d, &m).join(" ");
        assert!(p.contains("YouTube"), "dice qué no funciona aunque todo conecte: {p}");
    }

    #[test]
    fn quita_la_carpeta_personal() {
        assert_eq!(anonimizar("C:\\Users\\ana\\x\\navegador.exe", "C:\\Users\\ana"), "%USUARIO%\\x\\navegador.exe");
        assert_eq!(anonimizar("texto", ""), "texto", "una carpeta vacía no cambia nada");
    }

    #[test]
    fn prueba_directa_contra_un_servidor_local() {
        let puerto = servidor();
        let r = probar_directo(&format!("http://127.0.0.1:{puerto}/"));
        assert!(r.dns.is_ok(), "{:?}", r.dns);
        assert!(matches!(r.tcp, Some(Ok(_))), "{:?}", r.tcp);
        let (estado, _, desfase) = r.http.expect("la petición local funciona");
        assert_eq!(estado, 200);
        // El servidor de la prueba no manda `Date`: no hay desfase que medir.
        assert_eq!(desfase, None);
    }

    #[test]
    fn prueba_directa_contra_un_puerto_cerrado_dice_por_que() {
        let cerrado = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let r = probar_directo(&format!("http://127.0.0.1:{cerrado}/"));
        assert!(matches!(r.tcp, Some(Err(_))), "{:?}", r.tcp);
        assert!(r.http.is_err());
        assert!(pistas(&[r], &[]).join(" ").contains("cortafuegos"));
    }

    #[test]
    fn el_informe_trae_todas_las_secciones() {
        let d = [directa("https://a.com/", Ok(()), Some(Ok(())), Ok((200, Some(1))))];
        let m = [motor("https://a.com/", Ok("https://a.com/ — «A»"))];
        let t = informe(Path::new("/usr/bin/navegador"), &d, &m);
        for parte in ["DIAGNÓSTICO DEL NAVEGADOR", "1) RED DIRECTA", "2) CON EL MOTOR COMPLETO", "3) PISTAS", "4) ÚLTIMAS LÍNEAS DEL REGISTRO", "Versión:", "https://a.com/"] {
            assert!(t.contains(parte), "falta «{parte}» en:\n{t}");
        }
    }
}
