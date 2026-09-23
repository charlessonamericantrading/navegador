# ADR 0001 — Broker y aislamiento de renderers

- **Estado:** aceptada la etapa 1; etapas 2 y 3 propuestas.
- **Fecha:** 23-09-2026.
- **Plan:** F06, F07, F09; hallazgos H01 y H23; tarea 13 del backlog.
- **Modelo de amenazas:** [`SECURITY.md`](../../SECURITY.md).

## Contexto

Hoy hay **un** proceso `engine_server` para todas las pestañas. Ese proceso
interpreta contenido hostil (HTML, CSS, JavaScript, imágenes, fuentes) y, a la
vez, tiene todo lo valioso:

- hace su propia red (`engine-net`: hyper + rustls) con las cookies de todos los
  sitios;
- lee y escribe en disco `cookies.json` y `local_storage.json` bajo
  `%APPDATA%/navegador-ia` (o su equivalente);
- corre con los permisos del usuario: `sandbox.rs` aplica mitigaciones de
  Windows (sin código dinámico, sin procesos hijo, sin puntos de extensión) y
  declara expresamente que **no** es un sandbox.

Consecuencias:

1. Un fallo explotable en Boa, html5ever, el decodificador de imágenes o el
   layout da acceso a las cookies y al almacenamiento de todos los sitios y a
   los ficheros del usuario. Rust reduce ciertas clases de fallo, pero hay
   dependencias con avisos abiertos (`fast-float` por Boa) y código `unsafe` en
   la cadena.
2. Un cuelgue o pánico en una pestaña se lleva todas (H23: además, el proceso
   nativo no tiene reinicio supervisado).
3. Pestaña y frontera de seguridad no son lo mismo: el destino es aislar por
   sitio, no por pestaña (ver el diseño de *site isolation* de Chromium).

## Decisión

Separar en tres etapas, cada una utilizable y reversible por sí sola.

### Etapa 1 — Un proceso de renderer por pestaña, supervisado (se implementa ahora)

- El proceso principal de Electron hace de **supervisor**: arranca un
  `engine_server` por pestaña, enruta cada petición al proceso de su pestaña y
  detecta caídas.
- La caída de un renderer se **contiene**: se rechazan sus peticiones
  pendientes, la pestaña queda marcada como caída y las demás siguen
  respondiendo. La interfaz ofrece recargarla.
- Cada renderer sigue haciendo su red y su disco. **Esta etapa aporta
  contención de fallos y de cuelgues, no confidencialidad**: un renderer
  comprometido todavía lee el disco del usuario.
- La lógica vive en un módulo propio (`desktop/engine-supervisor.js`) con una
  interfaz pequeña (abrir, pedir, cerrar, eventos de caída) para poder
  sustituir su implementación por un broker Rust sin tocar la interfaz.

### Etapa 2 — Broker con las capacidades; renderer sin permisos

- Red, cookies, almacenamiento, descargas y lectura de ficheros elegidos por el
  usuario pasan a un **broker** (proceso Rust aparte o el supervisor).
- El renderer pide recursos por un canal acotado y autenticado por contexto
  (pestaña, documento, origen); el broker aplica CORS, CSP, cookies y
  particiones y decide.
- Con el renderer sin disco ni red propios, se puede restringir su token:
  en Windows, token restringido o AppContainer más un *Job Object* (sin procesos
  hijo, límite de memoria, cierre del árbol al morir el padre); en Linux,
  espacios de nombres y seccomp; en macOS, el sandbox del sistema.
- **Un fallo al aplicar la sandbox impide arrancar contenido no confiable**
  (principio 7 del plan): nada de continuar sin ella en silencio.

#### Progreso de la etapa 2

- **Decidido (23-09-2026):** el broker es un **proceso Rust propio**
  (`engine_broker`) que reutiliza `engine-net`, no el proceso principal de
  Electron. Así la política de red, cookies, CORS y CSP sigue en un solo sitio
  y el motor no queda atado a Electron.
- **Hecho — la costura (Fase 64):** `engine_net::ResourceBroker` reúne todo lo
  que el renderer pide al exterior: `fetch`, cookies desde JavaScript y seis
  operaciones de Web Storage por origen. El motor entero (navegación, `fetch`,
  `XHR`, `document.cookie`, `localStorage`) depende ya de esa interfaz y no de
  `NetworkEngine`/`WebStorage`. La única implementación, `LocalBroker`, hace lo
  mismo que antes en el mismo proceso.
- **Siguiente:** el binario `engine_broker` que sirve esa interfaz, un
  `RemoteBroker` que la implemente sobre un canal, y la regla que da sentido a
  todo: el broker sabe el origen del documento de cada renderer (porque hace él
  la navegación) y rechaza cookies o almacenamiento de cualquier otro origen.
- **Después:** con el renderer sin red ni disco propios, restringir su token.

### Etapa 3 — Aislamiento por sitio

- La unidad de aislamiento pasa a ser el sitio (esquema + dominio registrable),
  no la pestaña: dos pestañas del mismo sitio pueden compartir proceso y un
  `<iframe>` de otro sitio va a otro proceso (F09, F24).

## Alternativas descartadas

| Alternativa | Por qué no |
|---|---|
| Seguir con un proceso y más mitigaciones | Ya está hecho (Fase 23) y no cambia que el proceso que interpreta contenido hostil tenga las cookies y el disco. |
| Que `engine_server` lance sus propios hijos | Choca con la mitigación que prohíbe procesos hijo, que es una de las pocas defensas reales de hoy; y el supervisor tiene que sobrevivir a la caída del renderer, así que no puede ser él. |
| Broker Rust directamente, sin etapa 1 | Es la etapa 2 entera de golpe: mover red y almacenamiento cambia `engine-net`, `scripting` y el protocolo a la vez. La etapa 1 fija antes el contrato del supervisor y la contención, que la etapa 2 necesita igual. |
| Un proceso por sitio desde el principio | Exige saber qué documento y qué frames van a qué sitio (F09), y el motor todavía no tiene `<iframe>`. Se deja para la etapa 3. |

## Consecuencias

- **Memoria:** cada pestaña paga un proceso (fuentes, runtime de Boa). Se medirá
  con F39; si es un problema, la etapa 3 permite compartir por sitio.
- **Estado compartido:** cookies y `localStorage` viven hoy en cada proceso y se
  guardan en los mismos ficheros. Con varios procesos, **dos pestañas pueden
  pisarse al escribirlos**. La etapa 1 lo acepta de forma declarada (último que
  escribe gana); la etapa 2 lo resuelve al llevar el estado al broker. Mientras
  tanto no se anuncia la etapa 1 como segura para sesiones con credenciales en
  varias pestañas.
- **Protocolo:** las peticiones de pestañas (`new_tab`, `switch_tab`,
  `close_tab`, `list_tabs`) las atiende el supervisor, no el motor, cuando la
  etapa 1 se conecte a la interfaz.

## Criterio de aceptación de la etapa 1 (experimento)

Dos renderers vivos a la vez contra el binario real; se mata uno a mitad de
uso; el otro sigue respondiendo `ping` y navegando; las peticiones pendientes
del muerto se rechazan con un error de caída, no por timeout; y el supervisor
lo notifica. Probado con `node --test` contra `engine_server`.
