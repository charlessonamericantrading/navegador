#!/usr/bin/env node
// Trinquete de lint: falla si el numero de hallazgos SUBE, no si es mayor que
// cero.
//
// Por que asi y no exigiendo cero. Cuando este trinquete se escribio, `eslint`
// acababa de volver a funcionar (ver la Fase 46 en `engine/ARCHITECTURE.md`:
// llevaba roto porque `typescript-eslint` no soporta TypeScript 7) y encontro
// 20 hallazgos de golpe. Exigir cero habria dejado el CI en rojo hasta
// arreglarlos todos, y varios son de `react-hooks/exhaustive-deps`, cuyo
// "arreglo" puede cambiar el comportamiento de la interfaz. Un CI rojo
// permanente deja de leerse, que es peor que no tenerlo.
//
// Es el mismo patron que `crates/core/tests/api_probe.rs` usa para la sonda de
// APIs del motor, y por la misma razon.
//
// Al arreglar hallazgos, BAJA el numero de `BASE`. Nunca se sube.
//
// Usa la API de `eslint` y no lanza `npx`: lanzar un `.cmd` desde Node en
// Windows falla con EINVAL, y ademas el proceso extra no aporta nada.

import { ESLint } from 'eslint';

/** Hallazgos conocidos el 2026-09-09. Solo puede bajar. */
const BASE = 20;

const eslint = new ESLint();
const resultados = await eslint.lintFiles(['.']);

const porRegla = new Map();
let total = 0;
for (const fichero of resultados) {
  for (const m of fichero.messages) {
    total += 1;
    const regla = m.ruleId ?? '(sin regla)';
    porRegla.set(regla, (porRegla.get(regla) ?? 0) + 1);
  }
}

console.log(`\n=== Lint: ${total} hallazgos (base registrada: ${BASE}) ===`);
for (const [regla, n] of [...porRegla].sort((a, b) => b[1] - a[1])) {
  console.log(`  ${String(n).padStart(3)}  ${regla}`);
}

// El detalle completo solo cuando empeora: en verde, el resumen basta y el log
// de CI se mantiene legible.
if (total > BASE) {
  const formateador = await eslint.loadFormatter('stylish');
  console.error(await formateador.format(resultados));
  console.error(
    `El lint EMPEORO: ${total} hallazgos frente a los ${BASE} registrados.\n` +
      'Arregla lo que hayas anadido. Si el aumento es legitimo, sube `BASE` en\n' +
      'este fichero y explica por que en el mensaje del commit.'
  );
  process.exit(1);
}

if (total < BASE) {
  console.log(
    `\nMejoro: ${total} frente a ${BASE}. Baja \`BASE\` a ${total} en\n` +
      'scripts/lint-ratchet.mjs para que no pueda volver a subir.'
  );
}
