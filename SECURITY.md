# Seguridad del Navegador

**Estado: prototipo. No está preparado para navegar por sitios no confiables
con cuentas reales.** Este documento dice por qué, qué protege hoy y qué no, y
cómo avisar de un fallo. Se revisa en cada fase que toque una frontera de
seguridad (plan, sección 7.3).

Última revisión: 25-11-2026 (aplicación 100 % Rust, Fase 5 del plan).

## Cómo avisar de una vulnerabilidad

No abras un *issue* público. Usa un aviso privado del repositorio (GitHub
Security Advisories, «Report a vulnerability») o contacta en privado con quien
lo mantiene. Incluye versión o commit, sistema operativo y pasos para
reproducir. Se responde primero con el alcance y después con la corrección.

## Qué se protege

| Activo | Dónde vive hoy |
|---|---|
| Cookies y `localStorage` de todos los sitios | Proceso `engine_broker` (Fase 67), el único que abre `cookies.json` y `local_storage.json` bajo el directorio de datos del usuario |
| Clave del proveedor de IA | Todavía no hay agente en la aplicación Rust (Fase 5.4). Cuando lo haya, la clave irá al almacén de credenciales del sistema, nunca a un fichero ni a la URL |
| Ficheros del usuario | Accesibles con los permisos del usuario por cualquier proceso de la aplicación |
| Integridad de lo que ejecuta el agente | Pendiente con el agente (Fase 5.4) |
| Actualizaciones | No hay actualizador: se compila desde el código |

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
| Página → motor | Rust; parsers mantenidos (html5ever, rustls); mitigaciones de proceso de Windows (sin código dinámico, sin procesos hijo, sin puntos de extensión — Fase 23); CSP, CORS, SameSite y cookies `HttpOnly` aplicadas por el motor | **Sin sandbox**: el proceso que interpreta la página ya no *usa* red ni perfil propios (los pide al broker, que solo le da cookies y almacenamiento de orígenes a los que navegó — Fases 65–67), pero sigue corriendo con los permisos del usuario, así que **un motor comprometido todavía podría abrir el perfil o la red por su cuenta**. **Todas las pestañas comparten proceso.** (Los fallos de memoria de `fast-float` que arrastraba Boa 0.19 se cerraron migrando a Boa 0.22, Fase 62.) |
| Motor → ventana | Líneas limitadas en el motor (1 MiB), UTF-8 validado, el motor no muere con entrada malformada; la ventana lee cada línea como JSON y descarta lo que no entiende. Si el motor cae, la ventana sigue y lo dice | Sin contrapresión |
| Barra y páginas propias | Las pinta el motor a partir de HTML propio; el texto que viene de fuera (títulos, URL, mensajes de error) se escapa antes | — |
| Agente | Pendiente (Fase 5.4) | Hará falta autorización por acción sensible y defensas contra inyección de instrucciones |
| Red | TLS con rustls actualizado (RUSTSEC-2026-0285 corregido); esquemas no permitidos rechazados | Sin HSTS completo ni particionado de estado por sitio |
| Distribución | Se compila desde el código con `--locked`; prueba de humo de la app en CI | **Sin instalador ni firma** todavía (Fase 5.5) |

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
