#!/bin/bash

echo "==================================================="
echo "  Instalador del Navegador para Agentes de IA"
echo "==================================================="
echo ""

# Verificar si Node.js está instalado
if ! command -v node &> /dev/null; then
    echo "[ERROR] Node.js no está instalado. Por favor instálalo desde https://nodejs.org/"
    exit 1
fi

# Verificar si Python 3 está instalado
if ! command -v python3 &> /dev/null && ! command -v python &> /dev/null; then
    echo "[ERROR] Python no está instalado. Por favor instálalo desde https://www.python.org/"
    exit 1
fi

echo "[1/3] Instalando dependencias de Node en la raíz..."
npm install
if [ $? -ne 0 ]; then
    echo "[ERROR] Falló la instalación de dependencias en la raíz."
    exit 1
fi

echo "[2/3] Ejecutando instalación completa (Frontend, Backend y Electron)..."
echo "Esto puede tomar unos minutos..."
npm run install:all
if [ $? -ne 0 ]; then
    echo "[ERROR] Falló la instalación de dependencias secundarias."
    exit 1
fi

echo "[3/3] Creando acceso directo de ejecución..."
cat << 'EOF' > iniciar.sh
#!/bin/bash
cd "$(dirname "$0")"
echo "Iniciando aplicación..."
npm start
EOF

chmod +x iniciar.sh

echo ""
echo "==================================================="
echo "  ¡Instalación completada con éxito!"
echo "==================================================="
echo ""
echo "Ahora puedes ejecutar el archivo './iniciar.sh'"
echo "en esta misma carpeta para arrancar la aplicación."
echo ""
