# Reuniones

Aplicación de escritorio para macOS que permite grabar reuniones de Microsoft Teams, volver a escuchar el audio y convertir las conversaciones en transcripciones, resúmenes y marcadores.

Está construida con **Rust y GPUI Kit**. La grabación y la transcripción se procesan en tu Mac; para generar resúmenes y marcadores puedes utilizar **Codex, GitHub Copilot o Claude Code** mediante sus CLI.

## Contenido

- [Requisitos](#requisitos)
- [Compilar y ejecutar](#compilar-y-ejecutar)
- [Primer uso](#primer-uso)
- [Funcionalidades](#funcionalidades)
- [Configuración](#configuración)
- [Atajos de teclado](#atajos-de-teclado)
- [Archivos y privacidad](#archivos-y-privacidad)
- [Desarrollo](#desarrollo)

## Requisitos

| Requisito | Uso |
| --- | --- |
| macOS 14.2 o posterior | Captura de audio de Teams mediante Core Audio Process Taps. |
| Rust y Cargo | Compilación de la aplicación. |
| Herramientas de línea de comandos de Xcode y CMake | Compilación de las dependencias nativas. |
| Microsoft Teams de escritorio | Fuente del audio de la reunión. |
| CLI de Codex, GitHub Copilot o Claude Code | Opcional, para resúmenes y marcadores con IA. |

La transcripción utiliza la GPU de tu Mac para acelerar el procesamiento.

La captura está implementada para **Teams de escritorio en macOS**. No captura llamadas de Teams desde el navegador ni de otras aplicaciones.

## Compilar y ejecutar

Desde la raíz del repositorio:

```sh
./scripts/bundle-macos
open target/Reuniones.app
```

El script compila en modo release, crea el paquete `.app` y lo firma localmente. Si defines `CARGO_TARGET_DIR`, el paquete se genera en ese directorio.

Para una compilación de desarrollo:

```sh
./scripts/bundle-macos --debug
open target/Reuniones.app
```

Si la terminal no reconoce `cargo`, instala Rust o carga su entorno antes de compilar:

```sh
source "$HOME/.cargo/env"
```

## Primer uso

1. Abre **Configuración → Transcripción** y descarga el modelo de Whisper, de aproximadamente **547 MiB**. Solo necesitas descargarlo una vez.
2. Si quieres usar IA, inicia sesión en una de las CLI y selecciónala en **Configuración → Inteligencia artificial**.
3. Abre Teams de escritorio y entra a la llamada antes de grabar.
4. Comprueba la entrada de audio predeterminada de macOS y pulsa el **icono rojo de grabación**.
5. Concede los permisos de micrófono y captura de audio del sistema cuando macOS los solicite.
6. Añade marcadores mientras grabas, pausa o reanuda cuando lo necesites y pulsa **Detener** al terminar.
7. Cuando se guarde el audio, comenzará la transcripción si el modelo ya está instalado. Después se ejecutarán las automatizaciones que hayas activado.

El inicio y el fin de la grabación se controlan desde Reuniones: **la app no detecta automáticamente que la llamada de Teams terminó**. Mantén la aplicación abierta mientras transcribe o genera contenido.

## Funcionalidades

### Grabación

- Captura simultánea del **micrófono** y del **audio que reproduce Teams**, también al usar auriculares.
- Pistas separadas y sincronizadas, más una mezcla para la reproducción.
- Controles de inicio, pausa, reanudación y finalización.
- Contador del tiempo grabado e indicadores de señal para micrófono y Teams.
- Avisos si no se detecta señal o se interrumpe la captura.
- Marcadores manuales durante la grabación.
- Finalización y guardado del audio pendiente al salir de la aplicación.

Se utiliza el micrófono predeterminado de macOS. **Silenciarte en Teams no detiene la grabación de tu micrófono en Reuniones.**

### Biblioteca de reuniones

- Lista de reuniones con **nombre, fecha y hora locales de grabación y duración**.
- Cambio de nombre mediante el icono de lápiz junto al título.
- Eliminación con confirmación: borra el audio, la transcripción, el resumen y los marcadores de esa reunión.
- Reordenación por **arrastrar y soltar**, el orden personalizado se conserva entre sesiones.
- Búsqueda por **nombre de la reunión o contenido de la transcripción**.

El buscador encuentra coincidencias parciales sin distinguir mayúsculas y minúsculas. No busca en resúmenes, marcadores ni fechas. Mientras haya una búsqueda activa, la reordenación queda deshabilitada, tanto por arrastre como por teclado.

### Reproducción

- Reproducir y pausar la mezcla de micrófono y Teams.
- Retroceder o avanzar **15 segundos**.
- Cambiar la posición desde la barra de progreso, con actualización del tiempo durante el arrastre.
- Saltar a un momento desde la transcripción, un marcador o una referencia del resumen.
- Añadir marcadores en la posición actual.

### Transcripción local

- Transcripción configurada en **español** mediante `whisper-rs` y `whisper.cpp`, con aceleración Metal.
- Modelo **Whisper large-v3-turbo**, cuantizado en formato `q5_0`.
- Descarga desde la configuración, con verificación SHA-256.
- Generación automática después de guardar la grabación, si el modelo está instalado, o manual desde la pestaña **Transcripción**.
- Segmentos con tiempos de inicio y fin, y texto seleccionable para copiar.
- Identificación de la pista: **Tú** para el micrófono y **Teams** para el audio de la llamada.
- Paginación para recorrer transcripciones largas.

La etiqueta Teams representa la pista conjunta de la llamada; no identifica por separado a cada participante. Una vez descargado el modelo, la transcripción no necesita conexión a Internet.

### Resúmenes con IA

- Generación manual o automática a partir de la transcripción.
- Resumen con **decisiones, tareas, responsables, fechas explícitas y preguntas abiertas**, según lo que aparezca en la conversación.
- Referencias a segmentos para volver al audio que respalda cada punto.
- Texto seleccionable y edición del **texto principal del resumen**.
- Regeneración con confirmación antes de reemplazar el contenido y sus ediciones.
- Instrucciones personalizadas para ajustar el enfoque, el detalle y el idioma del resultado.

### Marcadores

- Creación manual al grabar o al revisar el audio.
- Cambio de nombre con el lápiz junto a cada marcador.
- Generación con IA de marcadores para los temas principales de la reunión.
- Generación automática después de la transcripción, independiente de los resúmenes automáticos.
- Validación de los tiempos generados contra el inicio de los segmentos de la transcripción.
- Regeneración con confirmación: reemplaza los marcadores generados anteriormente y **conserva los manuales y los que hayas renombrado**.
- Instrucciones personalizadas para elegir qué momentos destacar y cómo nombrarlos.

La generación con IA requiere una transcripción con diálogo. Si no hay temas identificables, puede devolver una lista sin marcadores.

## Configuración

### Apariencia

Selecciona el tema **Claro** u **Oscuro**. La elección se aplica a toda la aplicación y queda guardada para las próximas sesiones.

### Inteligencia artificial

Elige la herramienta que utilizas e inicia sesión primero desde la terminal:

| Herramienta | Inicio de sesión |
| --- | --- |
| Codex | `codex login`, con tu cuenta de ChatGPT. |
| GitHub Copilot | `copilot login`. |
| Claude Code | `claude auth login`. |

Las herramientas se ejecutan mediante sus **CLI locales**. Cada CLI gestiona su autenticación y el acceso disponible en tu cuenta; la aplicación no extrae sus tokens. Usa una versión actualizada de la herramienta seleccionada.

Puedes elegir **modelo y esfuerzo de razonamiento**. Las preferencias se guardan por herramienta. Con **Predeterminado de la herramienta**, no se envía una selección explícita para ese valor. En Claude Code, Sonnet, Opus y Haiku son alias que resuelve la CLI según su configuración y disponibilidad.

#### Instrucciones de generación

Hay dos editores independientes:

- **Resumen:** define el contenido, el enfoque, el nivel de detalle y el idioma del resumen.
- **Marcadores:** define los temas o momentos que quieres destacar y el estilo de las etiquetas.

Cada editor permite **Guardar**, **Cancelar** o **Restablecer** las instrucciones predeterminadas. Restablecer modifica el borrador del editor; debes guardar para aplicar el cambio. Dejar el editor vacío y guardar también recupera el valor predeterminado.

Las instrucciones se aplican a las próximas generaciones **manuales y automáticas** con cualquiera de los proveedores. No modifican los resultados ya guardados. La aplicación sigue gestionando el formato de respuesta y la validación de referencias y tiempos.

#### Al terminar una reunión

Puedes activar por separado:

- **Resúmenes automáticos.**
- **Marcadores automáticos.**

Ambos se ejecutan después de guardar el audio y completar la transcripción. Si activas los dos, primero se genera el resumen y después los marcadores. Al cambiar de proveedor, las automatizaciones se desactivan para que puedas confirmar su uso con la nueva herramienta.

### Transcripción, grabación y archivos

Desde la configuración también puedes descargar el modelo de Whisper, consultar cómo se captura el micrófono y utilizar **Abrir en Finder** para acceder a la carpeta de reuniones.

## Atajos de teclado

| Atajo | Acción |
| --- | --- |
| `⌘⇧M` | Añadir un marcador durante la grabación o en la posición de la reunión seleccionada. |
| `⌘⌥↑` | Subir la reunión que tiene el foco en la lista. |
| `⌘⌥↓` | Bajar la reunión que tiene el foco en la lista. |

La reordenación por teclado requiere que el foco esté en un elemento de la lista y que el buscador esté vacío.

## Archivos y privacidad

Los datos se guardan en:

```text
~/Library/Application Support/Reuniones/
├── settings.json                     # Preferencias, prompts y orden de las reuniones
├── ggml-large-v3-turbo-q5_0.bin        # Modelo de Whisper
└── meetings/
    └── <id-de-la-reunión>/
        ├── meeting.json              # Datos, transcripción, resumen y marcadores
        ├── mic.wav                   # Pista del micrófono
        ├── teams.wav                 # Pista de Teams
        └── meeting.wav               # Mezcla para reproducción
```

- **Audio y transcripción:** se procesan y guardan localmente.
- **Resúmenes con IA:** el proveedor recibe la transcripción y las referencias de sus segmentos.
- **Marcadores con IA:** el proveedor recibe el texto con sus tiempos, sin el campo que identifica la pista de audio. Esto no elimina los nombres u otros datos que se hayan mencionado en la conversación.
- **Audio:** la aplicación no lo envía al proveedor de IA.
- **Configuración:** los cambios se guardan localmente en `settings.json`.

La conexión a Internet se necesita para descargar el modelo y utilizar los proveedores de IA.

## Desarrollo

### Componentes principales

| Archivo | Responsabilidad |
| --- | --- |
| `src/main.rs` | Inicio, ventana, recursos gráficos y atajos. |
| `src/app.rs` | Estado de la aplicación y coordinación de los flujos. |
| `src/app/view.rs` | Biblioteca, reproductor, pestañas, configuración y controles. |
| `src/app/theme.rs` | Temas claro y oscuro. |
| `src/audio.rs` | Captura Core Audio, escritura de WAV, mezcla y reproducción con Rodio. |
| `src/processing.rs` | Descarga del modelo, transcripción y generación mediante las CLI. |
| `src/agent_options.rs` | Opciones de modelos y esfuerzo de los proveedores. |
| `src/library.rs` | Tipos de datos y persistencia JSON. |

### Verificaciones

```sh
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
```
