// Motor falso para probar el supervisor sin compilar Rust: habla el mismo
// NDJSON (saludo `ready`, `ping` -> `pong`) y tiene dos órdenes de prueba que
// el motor real no tiene: `crash` (termina con código 3) y `hang` (no
// contesta nunca).
const readline = require('readline');

process.stdout.write(`${JSON.stringify({ type: 'ready', id: 'boot', protocol_version: 1 })}\n`);
readline.createInterface({ input: process.stdin }).on('line', (line) => {
  const msg = JSON.parse(line);
  if (msg.type === 'ping') process.stdout.write(`${JSON.stringify({ type: 'pong', id: msg.id })}\n`);
  else if (msg.type === 'crash') process.exit(3);
  else if (msg.type === 'hang') { /* sin respuesta */ }
  else if (msg.type === 'shutdown') {
    process.stdout.write(`${JSON.stringify({ type: 'ok', id: msg.id, message: 'shutdown' })}\n`);
    process.exit(0);
  }
});
