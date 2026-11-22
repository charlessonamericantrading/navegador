// Que la app empaquetada lleve todos los ficheros que el proceso principal
// necesita. `electron-builder` solo copia los de `build.files`: un `require`
// de un fichero que falta no se nota con `npm test` (corre desde el código
// fuente) pero en la app instalada impide que la ventana llegue a abrirse. Así
// falló el trabajo `desktop` del CI con `ai-settings.js`.
//
//   npm test
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const raiz = path.join(__dirname, '..');
const paquete = JSON.parse(fs.readFileSync(path.join(raiz, 'package.json'), 'utf8'));

/** Todo fichero local que `entrada` pide con `require('./x')`, de forma transitiva. */
function alcanzables(entrada, vistos = new Set()) {
  if (vistos.has(entrada)) return vistos;
  vistos.add(entrada);
  const fuente = fs.readFileSync(path.join(raiz, entrada), 'utf8');
  for (const [, nombre] of fuente.matchAll(/require\('\.\/([\w.-]+?)(?:\.js)?'\)/g)) {
    alcanzables(`${nombre}.js`, vistos);
  }
  return vistos;
}

test('todo lo que main.js y preload.js necesitan está en build.files', () => {
  const necesarios = new Set([...alcanzables('main.js'), ...alcanzables('preload.js')]);
  const empaquetados = new Set(paquete.build.files);
  const faltan = [...necesarios].filter((fichero) => !empaquetados.has(fichero));
  assert.deepEqual(faltan, [], `la app empaquetada no arrancaría: faltan ${faltan.join(', ')} en build.files de package.json`);
});

test('build.files no lista ficheros que no existen', () => {
  const sobran = paquete.build.files.filter((fichero) => fichero !== 'package.json' && !fs.existsSync(path.join(raiz, fichero)));
  assert.deepEqual(sobran, []);
});
