# Mediciones del motor

Resultados de `npm run bench` (`scripts/bench/run-corpus.mjs`) sobre el corpus
de `engine/tests/corpus/`. Cada JSON guarda **todas** las muestras, los fallos
con su motivo y el entorno (commit, hash del binario, SO, CPU, versiones).

Esto es una línea base del motor solo. **No hay comparación con Chrome**: eso
exige el protocolo completo de la sección 6.4 del plan (F39) en equipos y
condiciones controladas.

## Línea base del 23-09-2026

Dos ejecuciones completas seguidas, 20 repeticiones por página, en el mismo
equipo (Windows 11, `engine_server` en release). Ficheros:
`2026-09-23-32bfd80-ejecucion1.json` y `-ejecucion2.json`. El árbol tenía sin
confirmar el cambio de `NAVEGADOR_IA_PROFILE_DIR` que entra en el mismo commit
que estos ficheros; el binario medido lo incluye.

### Corrección: 10 de 12 páginas, igual en las dos ejecuciones

| Página | Categoría | Resultado | Motivo |
|---|---|---|---|
| estatica, formulario | carga | 20/20 | — |
| construir-dom, recorrer-nodos | dom | 20/20 | — |
| js-moderno, modulos-es | scripts | 20/20 y **0/20** | `modulos-es`: un módulo en línea que importa `./modulo-a.js` **no llega a ejecutarse**: el motor solo descarga los módulos de `<script src>` y `modulepreload`, no los `import` (hallazgo H10) |
| mini-framework | framework | **0/20** | `not a callable function`: **`element.querySelector` y `querySelectorAll` no existen**, solo en `document` (hallazgo H26, nuevo) |
| observadores | framework | 20/20 | — |
| temporizador, fetch-json, almacenamiento, eventos | resto | 20/20 | — |

Los dos fallos se comprobaron reduciéndolos a un caso mínimo para descartar
que el error estuviera en la página de prueba.

### Memoria: reproducible, y con una fuga

Memoria del proceso tras las 20 navegaciones de cada página, idéntica en las
dos ejecuciones:

| Página | MiB |
|---|---|
| estatica, formulario, modulos-es | 37–38 |
| js-moderno, almacenamiento, temporizador | 39–42 |
| observadores, eventos, fetch-json | 44–48 |
| mini-framework | 52 |
| recorrer-nodos | 60 |
| **construir-dom** (200 elementos) | **504** |

**La memoria no se libera entre navegaciones** (hallazgo H28, nuevo). Midiendo
`construir-dom` con 1, 5 y 10 repeticiones: 50, 148 y 267 MiB, unos 24 MiB más
por cada vez que se carga la página. El tiempo crece con ella (mediana de 53 a
157 ms). Las páginas sin JavaScript también crecen, pero poco. Algo de cada
documento abandonado —runtime de Boa, objetos de los elementos o sus
cierres— sigue vivo.

### Tiempos: NO concluyentes en este equipo

La misma página varía hasta 3 veces entre ejecuciones (mediana de
`estatica`: 27,3 ms y 11,8 ms; carga fría de `mini-framework`: 1.097 ms y
92 ms). El equipo no estaba en condiciones controladas. Con esa dispersión no se
publica ninguna cifra de velocidad como resultado (plan 6.4.9). Orientativo: en
caliente, casi todas las páginas responden en decenas de milisegundos, y
`construir-dom` es la única que pasa de 50.

## Cómo repetirlo

```bash
npm run build:engine
npm run bench                      # 20 repeticiones
npm run bench -- --reps 5 --only construir-dom,estatica
```

El motor corre con un perfil temporal (`NAVEGADOR_IA_PROFILE_DIR`): no lee ni
escribe cookies ni `localStorage` del perfil real.
