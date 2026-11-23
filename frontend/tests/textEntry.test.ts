// Qué elementos reciben el teclado al clicar sobre ellos (tarea 3.2).
//
//   npm test
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { isTextEntryElement } from '../src/domains/browser/textEntry.ts';

const el = (tag_name: string, element_type?: string) => ({ tag_name, attributes: { element_type } });

test('los campos de texto y los textarea reciben el teclado', () => {
  assert.equal(isTextEntryElement(el('input')), true, 'input sin type es de texto');
  for (const tipo of ['text', 'search', 'email', 'password', 'number', 'url', 'tel', 'TEXT']) assert.equal(isTextEntryElement(el('input', tipo)), true, tipo);
  assert.equal(isTextEntryElement(el('textarea')), true);
});

test('casillas, botones y demás controles no lo reciben', () => {
  for (const tipo of ['checkbox', 'radio', 'button', 'submit', 'reset', 'file', 'image', 'range', 'color', 'hidden']) assert.equal(isTextEntryElement(el('input', tipo)), false, tipo);
  for (const etiqueta of ['a', 'button', 'select', 'div']) assert.equal(isTextEntryElement(el(etiqueta)), false, etiqueta);
});
