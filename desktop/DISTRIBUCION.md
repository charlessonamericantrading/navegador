# Distribuir la app a público general

Esta app se reparte hoy como un instalador `.exe` sin firmar. Windows SmartScreen
mostrará el aviso "Windows protegió su PC" a cualquier persona que lo descargue,
lo cual asusta a usuarios no técnicos. Esto no se puede resolver solo con código:
requiere una decisión y una compra que solo tú puedes hacer.

## Opción A (recomendada si esperas muchas descargas): firmar el instalador

1. Compra un certificado de firma de código ("Code Signing Certificate", tipo
   OV o EV) con una autoridad como DigiCert, Sectigo o SSL.com. Cuesta del
   orden de 70-300 USD/año. Los EV tienen mejor reputación inmediata en
   SmartScreen, pero también cuestan más.
2. Exporta el certificado como archivo `.pfx` con contraseña.
3. En la máquina donde generes el build (`npm run build:app` /
   `electron-builder`), define estas variables de entorno antes de construir:
   - `CSC_LINK` → ruta al archivo `.pfx` (o su contenido en base64)
   - `CSC_KEY_PASSWORD` → la contraseña del `.pfx`

   electron-builder detecta estas dos variables automáticamente: no hace falta
   tocar `package.json` ni el script de build. **Nunca subas el `.pfx` ni la
   contraseña al repositorio** — solo deben existir en tu máquina de build o en
   un secreto de CI.

## Opción B (gratis, mientras tanto): avisar a los usuarios cómo continuar

Si no vas a firmar todavía, añade en tu página de descarga o README instrucciones
como:

> Windows puede mostrar "Windows protegió su PC" porque la app es nueva y no
> está firmada digitalmente. Pulsa "Más información" y luego "Ejecutar de todas
> formas" para continuar con la instalación.

Esto no sustituye a la firma, pero reduce el abandono de usuarios que no
entienden el aviso.

## Auto-actualización

El código ya usa `electron-updater` (ver `desktop/main.js`, función
`setupAutoUpdates`): al abrir la app empaquetada, comprueba si hay una versión
nueva y, si la descarga, avisa en la propia interfaz con un botón para
reiniciar e instalar. **No hace nada en modo desarrollo.**

Para que funcione de verdad, antes de tu primer lanzamiento público:

1. Decide dónde vas a alojar los releases. La opción más simple y gratuita es
   **GitHub Releases**.
2. Sube el proyecto a un repositorio de GitHub (hoy esta carpeta no es
   siquiera un repositorio git).
3. En `desktop/package.json`, dentro de `build.publish`, sustituye
   `"TU-USUARIO-DE-GITHUB"` y `"TU-REPOSITORIO"` por los valores reales.
4. Al ejecutar `npm run build:app` / `electron-builder`, añade `--publish always`
   (o define `GH_TOKEN` con un token de GitHub con permiso `repo`) para que
   suba automáticamente el instalador y el archivo `latest.yml` que
   `electron-updater` necesita para detectar versiones nuevas.

Hasta que completes estos pasos, la comprobación de actualizaciones fallará en
silencio (por diseño): no debe mostrarse como un error a quien esté usando la
app mientras tanto.
