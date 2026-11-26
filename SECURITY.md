# Seguridad del Navegador

**Estado: prototipo. No está preparado para navegar por sitios no confiables
con cuentas reales.** Este documento dice por qué, qué protege hoy y qué no, y
cómo avisar de un fallo. Se revisa en cada fase que toque una frontera de
seguridad (plan, sección 7.3).

Última revisión: 26-11-2026 (aislamiento en Windows del proceso `--motor`, instalador 100 % Rust sin firma).

## Cómo avisar de una vulnerabilidad

No abras un *issue* público. Usa un aviso privado del repositorio (GitHub
Security Advisories, «Report a vulnerability») o contacta en privado con quien
lo mantiene. Incluye versión o commit, sistema operativo y pasos para
reproducir. Se responde primero con el alcance y después con la corrección.

## Qué se protege

| Activo | Dónde vive hoy |
|---|---|
| Cookies, `localStorage`, historial y marcadores | El proceso `--broker` abre `cookies.json` y `local_storage.json` bajo `profile_dir()`. En Windows, ese directorio se endurece con `S:(ML;OICI;NWNR;;;ME)` para bloquear lectura y escritura desde el `--motor` en integridad baja; historial y marcadores viven en el supervisor UI |
| Clave del proveedor de IA | Todavía no hay agente en la aplicación Rust (Fase 5.4). Cuando lo haya, la clave irá al almacén de credenciales del sistema, nunca a un fichero ni a la URL |
| Ficheros del usuario | El supervisor y `--broker` corren como el usuario. En Windows, `--motor` corre con token restringido de integridad baja (`S-1-16-4096`): no puede modificar archivos ni claves de integridad media o alta (`NO_WRITE_UP`) y tiene *Job Object* con `ActiveProcessLimit = 1`, tope de memoria y bloqueo directo de portapapeles |
| Integridad de lo que ejecuta el agente | Pendiente con el agente (Fase 5.4) |
| Actualizaciones | Instalador/desinstalador propio 100 % Rust en usuario (`instalador`), sin actualizador automático ni firma digital todavía |

## Adversarios considerados

1. **Una página web hostil** — el principal. Controla HTML, CSS, JavaScript,
   imágenes y fuentes que el motor interpreta.
2. **Contenido que intenta dirigir al agente** (inyección de instrucciones): una
   página que pide al modelo revelar datos, comprar o cambiar permisos.
3. **La red**: redes hostiles y certificados inválidos.
4. **Una dependencia comprometida o con fallos conocidos.**
5. **Otra aplicación o página local** que intenta hablar con la aplicación
   (IPC, puertos locales).

Fuera de alcance por ahora: un atacante con acceso físico o con los mismos
permisos que el usuario en su equipo.

## Fronteras y lo que las protege hoy

```text
Página hostil ──► navegador --motor (Rust: parser, CSS, layout, Boa)
                      │  │ canal local autenticado (token de un solo uso)
                      │  └──► navegador --broker (red, cookies, almacenamiento, perfil)
                      │ NDJSON por stdin/stdout, 1 MiB por petición
                      ▼
               navegador (ventana, barra y pestañas; pinta con el mismo motor)
```

| Frontera | Protección actual | Hueco conocido |
|---|---|---|
| Página → motor | Rust; parsers mantenidos (`html5ever`, `rustls`, `boa_engine` 0.22); mitigaciones de proceso en Windows (sin código dinámico, sin procesos hijo, sin puntos de extensión, sin fuentes no confiables — Fase 23); **aislamiento del subproceso `--motor` en Windows** (`CreateRestrictedToken` `DISABLE_MAX_PRIVILEGE` + integridad baja `S-1-16-4096`, escritorio oculto dedicado, *Job Object* con memoria/procesos/portapapeles bloqueados y perfil inaccesible `S:(ML;OICI;NWNR;;;ME)`); CSP, CORS, SameSite y cookies `HttpOnly` | **Límites conocidos del sandbox**: fuera de Windows (Linux/macOS) sigue sin sandbox del SO. Incluso en Windows no usa *AppContainer* ni cortafuegos de sockets en el renderizador (no impide abrir conexiones directas ni leer archivos de usuario legibles por integridad baja fuera del perfil endurecido), y **todas las pestañas comparten un único proceso `--motor`** |
| Motor → ventana | Líneas limitadas en el motor (1 MiB), UTF-8 validado, el motor no muere con entrada malformada; la ventana lee cada línea como JSON y descarta lo que no entiende. Si el motor cae, la ventana sigue y lo dice | Sin contrapresión |
| Barra y páginas propias | Las pinta el motor a partir de HTML propio; el texto que viene de fuera (títulos, URL, mensajes de error) se escapa antes | — |
| Agente | Pendiente (Fase 5.4) | Hará falta autorización por acción sensible y defensas contra inyección de instrucciones |
| Red | TLS con rustls actualizado (RUSTSEC-2026-0285 corregido); esquemas no permitidos rechazados; pipe del broker autenticado por token de un solo uso y ACL de usuario + etiqueta baja para permitir conexión desde `--motor` | Sin HSTS completo ni particionado de estado por sitio |
| Distribución | Se compila desde el código con `--locked`; instalador 100 % Rust en usuario (`instalador` + `NavegadorIA-windows.zip`); pruebas de extremo a punta y de aislamiento en CI | **Sin firma de binarios** ni actualizador automático (Fase 5.5) |

## La consecuencia práctica

Un fallo explotable en el motor —el componente que procesa contenido hostil—
ya no permite en Windows modificar ficheros del usuario (`NO_WRITE_UP`), leer
el perfil endurecido (`cookies.json`, `local_storage.json`) ni lanzar `cmd.exe`
o subprocesos desde `--motor` (comprobado en `tests/sandbox_isolation.rs`). No
obstante, sigue faltando aislamiento de red en el propio renderizador (*AppContainer*),
aislamiento por proceso/sitio entre pestañas, sandbox en Linux/macOS y firma de
distribución. Por eso el navegador sigue marcado como prototipo: no se recomienda
usarlo con cuentas reales críticas frente a sitios no confiables.

El plan para cerrarlo está en
[`docs/adr/0001-broker-y-aislamiento-de-renderers.md`](docs/adr/0001-broker-y-aislamiento-de-renderers.md):
un proceso por pestaña supervisado (contención de caídas), después un broker
que se quede con red, cookies y disco para poder restringir el proceso que
interpreta la página, y por último aislamiento por sitio.

## Condiciones que bloquean una versión

Tomadas del plan (sección 5.3): escape de sandbox o lectura de datos de otro
contexto; credenciales expuestas; actualizaciones no autenticadas; dependencia
crítica sin tratar en una ruta distribuida; cancelación del agente ineficaz o
acción fuera de lo autorizado; un *harness* de pruebas que da éxito sin
ejecutar. Hoy se cumplen varias de ellas, por eso no hay versión estable.
