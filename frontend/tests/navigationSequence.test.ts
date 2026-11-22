// Numeración de navegaciones (tarea 1.3): una respuesta atrasada no pisa la pantalla.
//
//   npm test
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { NavigationSequence, isCancellationMessage, isNavigationCommand } from '../src/core/navigationSequence.ts';

test('la respuesta de una navegación es vigente mientras nadie pida otra', () => {
  const secuencia = new NavigationSequence();
  const lenta = secuencia.ticket('navigate');
  assert.equal(secuencia.isCurrent(lenta), true);
});

test('una navegación nueva deja obsoleta la anterior (lenta y luego rápida)', () => {
  const secuencia = new NavigationSequence();
  const lenta = secuencia.ticket('navigate');
  const rapida = secuencia.ticket('navigate');
  assert.equal(secuencia.isCurrent(lenta), false, 'la respuesta de la lenta se ignora cuando llega');
  assert.equal(secuencia.isCurrent(rapida), true);
});

test('un clic o un scroll no invalidan una navegación, pero una navegación nueva sí los invalida', () => {
  const secuencia = new NavigationSequence();
  const navegar = secuencia.ticket('navigate');
  const clic = secuencia.ticket('click');
  assert.equal(secuencia.isCurrent(navegar), true, 'un clic no es una navegación nueva');
  assert.equal(secuencia.isCurrent(clic), true);

  secuencia.ticket('back');
  assert.equal(secuencia.isCurrent(clic), false, 'el clic se mandó sobre una página que ya no es la vigente');
});

test('Detener deja obsoleto todo lo que hubiera en vuelo', () => {
  const secuencia = new NavigationSequence();
  const navegar = secuencia.ticket('navigate');
  const scroll = secuencia.ticket('scroll');
  secuencia.invalidateAll();
  assert.equal(secuencia.isCurrent(navegar), false);
  assert.equal(secuencia.isCurrent(scroll), false);
  assert.equal(secuencia.isCurrent(secuencia.ticket('navigate')), true, 'y lo siguiente vuelve a valer');
});

test('qué peticiones cambian de página', () => {
  for (const tipo of ['navigate', 'back', 'forward', 'new_tab', 'switch_tab', 'close_tab']) assert.equal(isNavigationCommand(tipo), true, tipo);
  for (const tipo of ['click', 'scroll', 'type_into_element', 'resize', 'get_state', 'stop']) assert.equal(isNavigationCommand(tipo), false, tipo);
});

test('reconoce el error con el que el motor contesta a una carga cancelada', () => {
  assert.equal(isCancellationMessage('navegacion_cancelada: la carga se detuvo o se pidio otra pagina'), true);
  assert.equal(isCancellationMessage('network_error: HTTP request failed'), false);
});
