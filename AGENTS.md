# Reglas del proyecto — leer antes de proponer nada

Este fichero lo leen los asistentes de IA (`AGENTS.md`, `CLAUDE.md` y
`GEMINI.md` tienen el mismo contenido). Son reglas del dueño del proyecto y no
se discuten.

## Reglas absolutas

1. **La aplicación es 100 % Rust.** El motor, la ventana, la interfaz (barra de
   direcciones, pestañas, panel del agente), el agente de IA y el instalador.
2. **Prohibido usar cualquier motor de navegador ya existente** (de Google,
   Apple, Mozilla o de cualquier otro), ni para pintar páginas, ni para la
   interfaz, ni para las pruebas, ni como "modo de compatibilidad". Tampoco
   instalarlo, sugerirlo ni nombrarlo. El objetivo del proyecto es crear el
   motor propio: depender de otro lo anula.
3. **Prohibido usar envoltorios de escritorio basados en tecnología web**
   (aplicaciones de escritorio hechas con HTML/JavaScript que llevan un
   navegador dentro). Ni usarlos, ni instalarlos, ni sugerirlos, ni nombrarlos.
4. Las carpetas `desktop/`, `frontend/` y `backend/` son de una etapa anterior
   que **incumple estas reglas** y se van a retirar (ver la Fase 5 de
   `PLAN_SONNET.md`). No se les añade nada y no se toman como modelo. Si una
   respuesta tuya se basa en ellas, está mal planteada.
5. Las pruebas no dependen de ningún navegador externo: se prueba con el propio
   motor (tests de Rust, `scripts/dev/engine_driver.py`, capturas del motor).

## Dónde está lo que sí vale

- `engine/` — el motor, escrito desde cero en Rust (red, HTML, CSS, layout,
  pintado, JavaScript con Boa, árbol de accesibilidad). Estado y cómo medirlo:
  `engine/ESTADO.md`.
- `engine/crates/gfx` ya tiene una ventana nativa en Rust (`winit` + `wgpu`):
  es el punto de partida de la aplicación 100 % Rust.
- Plan de trabajo: `PLAN_SONNET.md`.
