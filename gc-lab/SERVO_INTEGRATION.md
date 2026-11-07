# Dos experimentos de integración con Servo

## GC conjunto validado

El [protocolo de marcado conjunto](servo-native/JOINT_GC.md) ya verifica 1.500
ciclos, 3.000 botones nativos finalizados y dos navegaciones reales manteniendo
vivo el realm durante los ciclos. Incluye 89 errores recuperados y conserva un
nodo desconectado que sigue siendo alcanzable desde JS. El resultado está en
`native-gc-proof.json`; no usa release_globals para recoger los ciclos.

## Integración nativa actual

El fork de Servo 0.7.0 se ha compilado localmente con nuestro intérprete enlazado
dentro de `servo-script`. El parser HTML le entrega directamente el programa
experimental y el dispatcher DOM llama a nuestra closure. La prueba muestra
«Funciona» sin WebDriver, intérprete externo ni JavaScript de transporte.

El [adaptador, parche y comandos](servo-native/README.md) están separados del
experimento anterior. `native-proof.json` registra el resultado local y el
workflow compila el fork y conserva logs/PNG como `servo-native-interpreter`.
SpiderMonkey permanece para el DOM y su GC; no se ha sustituido todo el backend.
La pulsación de la prueba nativa es `HTMLElement::Click` desde Rust, sintética.

## Experimento anterior: puente fuera de proceso

## Resultado y alcance

El experimento compila nuestro puente Rust y usa el binario oficial de Servo
0.7.0 para cargar `servo-fixture/index.html`. WebDriver pulsa el botón real. La
closure registrada por el programa se ejecuta en nuestro intérprete y modifica
el textContent del DOM real. Servo pinta «Funciona» y produce button.png.

Es una integración FUERA de proceso. SpiderMonkey sigue dentro de Servo y ejecuta
el pequeño transporte DOM de WebDriver. No se ha sustituido SpiderMonkey, no se
ha enlazado el intérprete como backend interno ni se ha compilado un fork de Servo.
El componente nuevo sí se compila y ejecuta; el renderer es el binario oficial
verificado. Esta distinción es parte del resultado, no trabajo terminado implícito.

El script de la página usa `type="application/x-mwr-js"` para que Servo no ejecute
el programa por su cuenta. El runner lee ese texto, lo entrega al binario Rust y
sirve las operaciones DOM tipadas. No soporta todavía scripts normales de la web.

## Qué hemos comprobado realmente

1. La página muestra «Pulsar» con el programa aún inerte.
2. Nuestro intérprete parsea el programa, consulta el botón y registra su closure.
3. WebDriver hace un click nativo de elemento, no una llamada JS programática.
4. El hook de transporte encola un ID. El texto sigue siendo «Pulsar» ANTES de
   entregar el evento a Rust. Eso verifica que el hook no contiene la closure.
5. Al entregar el ID, el intérprete ejecuta su AST y solicita SET_TEXT. El DOM real
   cambia a «Funciona». Se captura el resultado renderizado.
6. Al retirar la raíz global, nuestro GC libera entorno, wrapper y closure; el
   adaptador retira el registro fuerte y el listener de transporte de Servo.
7. Otro click nativo ya no encola eventos. Ambos registros quedan a cero.

La captura y el report.json se conservan como artifact `servo-dom-bridge` en CI.
`servo-proof.json` es el resultado registrado localmente. No es una captura de
Chrome ni una imagen generada: button.png sale del endpoint screenshot de Servo.

Los ceros del registro remoto NO demuestran que todo el DOM nativo haya sido
recogido por SpiderMonkey. El botón sigue insertado en document y WebDriver aún
puede tener referencias. Demuestran la retirada de NUESTROS handles y hooks.
Al terminar se cierra la sesión y el proceso. El puente externo no valida la colección conjunta: esa prueba pertenece al
experimento nativo posterior descrito al principio de este documento.

## Reproducir en Ubuntu 24.04 x86_64

```sh
sudo apt-get update
sudo apt-get install -y libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 libgstreamer-plugins-bad1.0-0 libgstreamer-gl1.0-0 libegl1 libgl1-mesa-dri fonts-dejavu-core
cargo build --manifest-path gc-lab/Cargo.toml --bin servo_bridge --release --offline
python3 gc-lab/fetch_servo.py
python3 gc-lab/run_servo_bridge.py --servo gc-lab/.servo-test/servo/servoshell
```

No se requiere display: se usa headless, EGL surfaceless y renderizado software.
El runner configura directorios XDG locales, lanza un servidor HTTP y WebDriver
solo en loopback, y termina ambos procesos incluso ante error. El comando de
instalación es para reproducir fuera del entorno restringido: en la ejecución
local descargamos y extrajimos las bibliotecas necesarias en un prefijo temporal,
sin instalar paquetes globales.

El archivo de Servo se verifica con el SHA-256 fijado en fetch_servo.py:
`728eba1be1cc1851e05dfaa90e18f98ab8644dd2355bedb0b8a5e89795ebe333`.
El asset pertenece a v0.7.0, cuyo commit es
`aac43a3f31a259f04a574f5ec4e959c943ad7cc7`. El runner comprueba además
Servo/0.7.0 en el user-agent. El campo browserVersion del servidor WebDriver dice
0.0.1; no lo usamos como prueba de versión del binario.

## Contrato del adaptador

`DomAdapter` expone CREATE, QUERY, TAG, TEXT, SET_TEXT, LISTEN, CLICK y RELEASE.
No expone eval, contexto JS ni ejecución de source. Strings y selectores viajan
como datos UTF-8 hex; el runner usa scripts de transporte constantes y argumentos
WebDriver, sin interpolar los datos en código JavaScript.

Los IDs remotos se cachean para preservar la identidad de una misma consulta.
Las closures nunca se serializan al navegador: el único hook remoto encola un ID.
La retirada de handles sigue el GC de nuestros wrappers. Si RELEASE falla, los
IDs permanecen en una cola de liberación pendiente, se informa el error y la
siguiente colección reintenta. No se olvida un handle para presentar un falso cero.

Las asignaciones fallidas después de crear un nodo remoto lo liberan. Los errores
en la instalación de listeners revierten la arista local. Se prueban ambos casos,
fallos de escritura y liberación, identidad compartida y entrega de evento externo
sin sintetizar otro click. Son seis pruebas nuevas; la suite tiene 52 tests.
El corpus anterior de diez programas contra Node + mock sigue pasando.

Limitaciones: querySelector requiere una coincidencia y aún no implementa null;
solo se conecta click, sin propagación real del Event a nuestra VM. La entrega al
intérprete es coordinada por el runner después de la acción nativa, no una llamada
sincrónica dentro del event loop de Servo. Las garantías de errores y tiempos son
las de este experimento, no las de un navegador terminado. No hay sandbox nuevo,
concurrencia, navegación de la página ni promesas en el puente.

## Mapa de acoplamiento verificado en el código de Servo

Los enlaces apuntan al commit de la release, no a main. Las rutas, blobs y líneas
se registran también en servo-source-audit.json.

| Frontera | Evidencia en Servo 0.7.0 | Consecuencia |
| --- | --- | --- |
| Ejecución de scripts clásicos | [run_a_classic_script / JS_ExecuteScript](https://github.com/servo/servo/blob/aac43a3f31a259f04a574f5ec4e959c943ad7cc7/components/script/dom/globalscope/script_execution.rs#L133) | Cambiar el compilador/intérprete no es sustituir una crate |
| Carga de script HTML | [HTMLScriptElement::execute](https://github.com/servo/servo/blob/aac43a3f31a259f04a574f5ec4e959c943ad7cc7/components/script/dom/html/scripting/htmlscriptelement.rs#L975) | La selección del backend debe integrarse con carga y ejecución |
| Identidad y raíces DOM | [Reflector](https://github.com/servo/servo/blob/aac43a3f31a259f04a574f5ec4e959c943ad7cc7/components/script_bindings/reflector.rs#L47) | El objeto Rust refleja un JSObject y se traza con JSTracer |
| Representación de callbacks | [CallbackObject](https://github.com/servo/servo/blob/aac43a3f31a259f04a574f5ec4e959c943ad7cc7/components/script_bindings/callback.rs#L150) | Las closures extranjeras necesitan representación y raíces propias |
| Listeners | [EventListenerType](https://github.com/servo/servo/blob/aac43a3f31a259f04a574f5ec4e959c943ad7cc7/components/script/dom/event/eventtarget.rs#L165) | Los listeners actuales usan TracedCallback<EventListener> |
| Mutación aparentemente simple | [SetTextContent](https://github.com/servo/servo/blob/aac43a3f31a259f04a574f5ec4e959c943ad7cc7/components/script/dom/node/node.rs#L3842) | Incluso esta ruta recibe un JSContext mutable |
| API pública de embedding | [WebView::evaluate_javascript](https://github.com/servo/servo/blob/aac43a3f31a259f04a574f5ec4e959c943ad7cc7/components/servo/webview.rs#L731) | Evalúa source; no instala un backend JS alternativo |
| Generación WebIDL | [codegen](https://github.com/servo/servo/blob/aac43a3f31a259f04a574f5ec4e959c943ad7cc7/components/script_bindings/codegen/codegen.py#L5461) | Conversiones, callbacks y tracing dependen de tipos SpiderMonkey |

Esto demuestra un bloqueo estructural para un reemplazo inmediato, no que sea
imposible integrar otro motor. Tampoco demuestra por sí solo un defecto de Servo.

## Fronteras que quedan tras el experimento nativo

El adaptador nativo ya conecta comandos DOM en el hilo de script, leases nativas
y callbacks extranjeros identificados. Conserva SpiderMonkey para el DOM y
reutiliza DomAdapter con un préstamo temporal del contexto durante cada llamada.

Solo después abordaríamos un backend completamente independiente: selección del
ejecutor de scripts, representación completa de listeners, tracing incremental entre heaps,
conversión de valores WebIDL y contexto de ejecución/excepciones. La integración del subconjunto está compilada y probada; esa refactorización
completa aún no está hecha.
