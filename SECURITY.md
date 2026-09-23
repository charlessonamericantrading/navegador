# Seguridad de Navegador IA

**Estado: prototipo. No está preparado para navegar por sitios no confiables
con cuentas reales.** Este documento dice por qué, qué protege hoy y qué no, y
cómo avisar de un fallo. Se revisa en cada fase que toque una frontera de
seguridad (plan, sección 7.3).

Última revisión: 23-09-2026 (Fases 23 y 50 a 62 de `engine/ARCHITECTURE.md`).

## Cómo avisar de una vulnerabilidad

No abras un *issue* público. Usa un aviso privado del repositorio (GitHub
Security Advisories, «Report a vulnerability») o contacta en privado con quien
lo mantiene. Incluye versión o commit, sistema operativo y pasos para
reproducir. Se responde primero con el alcance y después con la corrección.

## Qué se protege

| Activo | Dónde vive hoy |
|---|---|
| Cookies y `localStorage` de todos los sitios | Proceso `engine_broker` (Fase 67), el único que abre `cookies.json` y `local_storage.json` bajo el directorio de datos del usuario |
| Clave del proveedor de IA | Proceso principal de Electron, cifrada con `safeStorage` (Fase 53) |
| Ficheros del usuario | Accesibles con los permisos del usuario por cualquier proceso de la aplicación |
| Integridad de lo que ejecuta el agente | Orquestador en la interfaz y comandos al motor |
| Actualizaciones | `electron-updater` hacia GitHub Releases (sin configurar todavía) |

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
Página hostil ──► engine_server (Rust: parser, CSS, layout, Boa)
                      │  │ canal local autenticado (token de un solo uso)
                      │  └──► engine_broker (red, cookies, almacenamiento, perfil)
                      │ NDJSON por stdin/stdout, 1 MiB por petición
                      ▼
               Proceso principal de Electron ──► proveedor de IA (clave en cabecera)
                      │ IPC: solo la ventana propia, esquema cerrado
                      ▼
               Interfaz React (app://, sin Node, aislamiento de contexto)
```

| Frontera | Protección actual | Hueco conocido |
|---|---|---|
| Página → motor | Rust; parsers mantenidos (html5ever, rustls); mitigaciones de proceso de Windows (sin código dinámico, sin procesos hijo, sin puntos de extensión — Fase 23); CSP, CORS, SameSite y cookies `HttpOnly` aplicadas por el motor | **Sin sandbox**: el proceso que interpreta la página ya no *usa* red ni perfil propios (los pide al broker, que solo le da cookies y almacenamiento de orígenes a los que navegó — Fases 65–67), pero sigue corriendo con los permisos del usuario, así que **un motor comprometido todavía podría abrir el perfil o la red por su cuenta**. **Todas las pestañas comparten proceso.** (Los fallos de memoria de `fast-float` que arrastraba Boa 0.19 se cerraron migrando a Boa 0.22, Fase 62.) |
| Motor → Electron | Líneas limitadas en las dos puntas, UTF-8 validado, el motor no muere con entrada malformada (Fase 54) | Sin contrapresión; un *timeout* no cancela el trabajo del motor |
| Interfaz → Electron | `contextIsolation`, sin `nodeIntegration`; `engine:request` y `ai:*` solo atienden a la ventana propia y a un esquema cerrado; `shutdown` no se puede pedir desde la página (Fases 53 y 54) | Sin CSP propia de la aplicación; fuentes externas en el arranque |
| `app://` | Resolución con `path.relative` sobre la URL parseada (Fase 54) | — |
| Clave de IA | Cifrada con `safeStorage`; solo en memoria si el sistema no cifra de verdad; en cabecera, nunca en la URL; errores redactados (Fase 53) | El backend Python opcional tiene su propia gestión |
| Agente | Detener cancela la petición y no actúa después; rellenar no envía; los fallos son fallos (Fases 51 y 52) | **Sin autorización por acción sensible** (compras, envíos, borrados); sin defensas específicas contra inyección de instrucciones |
| Red | TLS con rustls actualizado (RUSTSEC-2026-0285 corregido); esquemas no permitidos rechazados | Sin HSTS completo ni particionado de estado por sitio |
| Distribución | Integridad de ASAR (electron-builder 26); prueba de humo del paquete real en CI (Fase 57) | **Instalador sin firmar**; actualizaciones sin verificar de extremo a extremo |

## La consecuencia práctica

Un fallo explotable en el motor —el componente que procesa contenido hostil—
da acceso a las cookies de todos los sitios, al almacenamiento local y a los
ficheros del usuario. Que la red y el perfil estén ya en el broker (Fases
65–67) no cambia esto todavía: el motor comprometido puede abrir el fichero de
cookies directamente, porque corre como el usuario. Lo cambia restringir su
token (F07), que ahora es posible precisamente porque ya no los necesita. Es la razón de que no se recomiende iniciar sesión en
cuentas reales mientras se navega por sitios no confiables.

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
