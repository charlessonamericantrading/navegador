/** Lo que hace falta saber de un elemento de la lista del motor para decidir si recibe el teclado. */
export interface EntryCandidate {
  tag_name: string;
  attributes: { element_type?: string };
}

/**
 * ¿Es un campo donde se escribe texto? Un `<textarea>` y los `<input>` de texto
 * (y de búsqueda, correo, contraseña, número...) reciben las teclas; las
 * casillas, botones, ficheros, controles deslizantes y colores no.
 */
export function isTextEntryElement(el: EntryCandidate): boolean {
  if (el.tag_name === 'textarea') return true;
  if (el.tag_name !== 'input') return false;
  const type = (el.attributes.element_type ?? 'text').toLowerCase();
  return !['checkbox', 'radio', 'button', 'submit', 'reset', 'file', 'image', 'range', 'color', 'hidden'].includes(type);
}
