# Reuniones

Aplicación de escritorio en Rust y GPUI Kit para grabar reuniones de Microsoft Teams, escuchar el audio, marcar momentos y generar transcripciones y resúmenes en español.

## Ejecutar en macOS

Requiere macOS 14.2 o posterior, Rust, las herramientas de línea de comandos de Xcode y CMake. El modelo de Whisper se ha elegido para Macs con Apple Silicon.

```sh
./scripts/bundle-macos
open target/Reuniones.app
```

En **Configuración → Transcripción**, descarga el modelo de Whisper (547 MiB). Entra a la llamada en la aplicación de escritorio de Teams, verifica la entrada de audio predeterminada de macOS y selecciona **Grabar**. macOS solicitará acceso al micrófono y al audio del sistema. Durante la grabación, **⌘⇧M** añade un marcador.

En **Configuración → Resúmenes**, selecciona Codex, GitHub Copilot o Claude Code. Inicia sesión primero en la CLI correspondiente con `codex login`, `copilot login` o `claude auth login`. Puedes elegir modelo y esfuerzo, generar un resumen bajo demanda o activar **Generar resúmenes automáticamente**. El proveedor recibe el texto de la reunión. El audio y la transcripción se guardan en `~/Library/Application Support/Reuniones`.

Claude Code requiere una CLI actualizada y usa `claude -p` con la autenticación de la propia herramienta. Sonnet, Opus y Haiku son alias que resuelve la CLI según su configuración y el acceso de tu cuenta. Con «Predeterminado de la herramienta» se conserva su modelo y esfuerzo. La app solicita JSON estructurado, desactiva herramientas y MCP y evita guardar la sesión de resumen en el historial de Claude Code.
