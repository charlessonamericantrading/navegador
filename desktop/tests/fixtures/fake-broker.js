// Broker falso para probar `engine-broker.js` sin compilar Rust: habla el
// mismo canal de control (saludo `ready`, `register`, `revoke`, `shutdown`).
// `FAKE_BROKER_MODE` elige un fallo: `mudo` (nunca saluda), `muere` (termina
// al primer registro, ya saludado) o `rechaza` (contesta `error` a todo).
const readline = require('readline');

const mode = process.env.FAKE_BROKER_MODE || 'normal';
if (mode !== 'mudo') {
  process.stdout.write(`${JSON.stringify({ id: null, type: 'ready', endpoint: 'canal-falso' })}\n`);
}
readline.createInterface({ input: process.stdin }).on('line', (line) => {
  const msg = JSON.parse(line);
  const reply = (body) => process.stdout.write(`${JSON.stringify({ id: msg.id, ...body })}\n`);
  if (mode === 'muere') process.exit(4);
  if (mode === 'rechaza') return reply({ type: 'error', message: 'no me da la gana' });
  if (msg.type === 'register') reply({ type: 'registered', renderer: msg.renderer, token: 'f'.repeat(64) });
  else if (msg.type === 'revoke') reply({ type: 'revoked', renderer: msg.renderer });
  else if (msg.type === 'shutdown') {
    reply({ type: 'bye' });
    process.exit(0);
  }
});
