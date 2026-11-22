/**
 * Numeración de navegaciones (tarea 1.3 del plan).
 *
 * El motor contesta cada petición con su `state`, y una respuesta puede llegar
 * cuando ya no interesa: el usuario pidió otra página mientras la anterior
 * cargaba, o pulsó Detener. Aplicarla pintaba el contenido de la página vieja
 * bajo la dirección de la nueva. Regla: cada petición se manda con un número, y
 * su respuesta solo se aplica si ninguna navegación nueva (ni Detener) empezó
 * después.
 */

/**
 * Peticiones que cambian de página (o de pestaña). Empezar una invalida todo
 * lo que hubiera en vuelo. Un clic o un scroll no cuentan: no sabemos si un
 * clic navegará, y su respuesta es válida mientras nadie haya pedido otra cosa.
 */
const NAVIGATION_COMMANDS = new Set(['navigate', 'back', 'forward', 'new_tab', 'switch_tab', 'close_tab']);

export function isNavigationCommand(type: string): boolean {
  return NAVIGATION_COMMANDS.has(type);
}

/**
 * El motor cancela por su cuenta una carga sustituida o detenida y contesta a
 * esa petición con este error (ver `handle_interruptible` en el motor).
 */
export function isCancellationMessage(message: string): boolean {
  return message.startsWith('navegacion_cancelada');
}

export class NavigationSequence {
  private current = 0;

  /**
   * Número con el que se manda una petición. Una navegación abre un número
   * nuevo (y deja obsoleto todo lo anterior); cualquier otra petición hereda
   * el vigente.
   */
  ticket(commandType: string): number {
    if (isNavigationCommand(commandType)) this.current += 1;
    return this.current;
  }

  /** ¿Sigue siendo vigente la respuesta de esta petición? */
  isCurrent(ticket: number): boolean {
    return ticket === this.current;
  }

  /** Detener: todo lo que hubiera en vuelo queda obsoleto. */
  invalidateAll(): void {
    this.current += 1;
  }
}
