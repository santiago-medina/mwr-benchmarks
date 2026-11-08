# Integración nativa y marcado conjunto en Servo 0.7.0

El [protocolo de GC conjunto](JOINT_GC.md) ya se ha probado sobre ambos heaps
reales: 1.500 ciclos, 3.000 finalizaciones nativas y dos navegaciones sin
release_globals.

Este directorio contiene el adaptador dentro del hilo de script y un parche
mínimo sobre Servo fijado a `aac43a3f31a259f04a574f5ec4e959c943ad7cc7`.
El fork se ha compilado y la prueba local ha mostrado «Funciona» con
nuestro callback dentro de Servo. El resultado se registra en `../native-proof.json`;
el job `servo-native` reproduce la compilación y conserva los PNG y logs.

```sh
python3 gc-lab/servo-native/prepare.py
cd gc-lab/.servo-native
CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo +1.97.1 build -p servoshell --no-default-features --features bundled,js_jit --offline
cd ../..
python3 gc-lab/servo-native/run.py --servo gc-lab/.servo-native/target/debug/servoshell
```

Las dependencias nativas de Ubuntu están en el job `servo-native` del workflow.
El archivo fuente vendorizado se verifica con SHA-256 antes de extraerlo. El
parche se aplica con GNU patch, de forma independiente del repositorio Git
que contenga la descarga. El parche excluye `vendor` de la autodetección del workspace y registra cinco
paquetes ICU como miembros y dependencias locales. Así cbindgen usa el lockfile
principal al invocar cargo metadata, en vez de los lockfiles antiguos empaquetados
en esas crates. No se modifica código ni checksums de las dependencias vendorizadas.

`MWR_NATIVE_JS=1` habilita un único script inline `application/x-mwr-js` por
documento. El parser HTML entrega el texto directamente a nuestra VM. No hay
WebDriver, proceso de intérprete separado, RPC, eval de transporte ni serialización
de closures. El listener extranjero forma parte del dispatcher DOM de Servo.
Se conservan los controles previos de scripting y CSP de la ruta de preparación.
El estado de VM se suspende entre invocaciones y toma prestado el contexto DOM
solo durante la llamada; el adaptador no fabrica contextos. La VM mantiene
forbid(unsafe_code); los hooks nativos de trazado aíslan el unsafe necesario para
la API de Servo y sus invariantes se documentan en JOINT_GC.md.

`MWR_NATIVE_SELF_TEST=1` encola una tarea DOM para un turno posterior del
bucle de eventos. Esa tarea comprueba el texto inicial y pulsa con
`HTMLElement::Click` desde Rust, verifica «Funciona» y la
VM permanece viva. La recogida de ciclos se comprueba por separado con
`run_gc.py`, sin retirar las raíces globales. Es un evento
sintético del DOM real, no un clic físico. El runner solo arranca el navegador y
recoge logs y PNG; las operaciones y verificaciones DOM ocurren dentro de Servo.
Los controles adicionales deshabilitan el backend, esperan sin pulsar, bloquean
la ejecución mediante CSP y prueban sintaxis rechazada por nuestro parser.

Límites: SpiderMonkey sigue siendo necesario para reflectores, contexto y GC de
Servo. Los handles del registro son débiles; los pins `Trusted<Node>` solo duran
una invocación activa. El marcado conjunto está descrito en JOINT_GC.md. Solo hay click sin objeto Event en la VM; no se soportan scripts externos,
varios programas por documento, reentrada de callbacks ni `.click()` desde el
programa experimental. La navegación normal limpia handles/listeners antes de
destruir el runtime; el teardown tras un pánico solo descarta las raíces propias.
No es un motor JS compatible con la web ni un sandbox para páginas no confiables.

## Aplicación con objetos y compilaciones reutilizables

```sh
python3 gc-lab/servo-native/run.py \
  --servo gc-lab/.servo-native/target/debug/servoshell \
  --fixture gc-lab/servo-native/object-fixture.html \
  --before 'Cuenta: 0' --after 'Cuenta: 1' \
  --output gc-lab/native-object-results
```

El contador comprueba estado ordinario, getters/setters y el receptor real del
listener. Se ejecutan además los controles de backend deshabilitado, espera sin
click, CSP y sintaxis inválida. La prueba de 1.500 ciclos y dos navegaciones se
mantiene como regresión independiente.

Para evitar recompilar Servo en cada checkpoint, `servo-native` se activa con
`[servo]` en el mensaje del commit de push o mediante `workflow_dispatch`.
Los contratos siguen ejecutándose en commits normales; `[skip ci]` queda
para puntos de control incompletos o commits que solo archivan evidencia ya validada. La caché de Actions reutiliza el target
con la misma versión de Servo y Rust, y solo limpia los cuatro paquetes de
integración antes de compilarlos. No hay una ABI estable de crates Rust: cambiar
el toolchain o las features requiere una caché compatible o recompilar.

## Arrays e iteración

La [lista de tareas](../ARRAYS.md) usa tres controles y cuatro clicks sintéticos
para añadir, completar, eliminar y volver a añadir un nodo. `run.py` admite
`--target` y `--steps` para verificar el texto tras cada click; la orden exacta
está en el workflow. `task-fixture.html` contiene el mismo programa validado en
`examples/task-list.js`. `native-task-proof.json` y `native-array-gc-proof.json`
guardan el hito final, con la captura `native-task-list.png`.

La captura al nodo secundario de `gc-fixture.html` pasa ahora por un array y un
objeto, sin un binding directo alternativo al nodo. Se conservan los mismos
1.500 ciclos y las dos navegaciones para comprobar su trazado y reclamación.

## Promesas y cola de microtareas

[PROMISES.md](../PROMISES.md) documenta los jobs trazados, el límite de cada
checkpoint y la prueba `R → S → U` con un `MutationObserver` de SpiderMonkey.
Cada job VM se encola con `GlobalScope::enqueue_microtask` como un runnable real,
identificado solo por pipeline y serial. Los ámbitos `run_a_script` impiden
ejecutarlo dentro de una invocación activa. El parche en `runtime/job_queue.rs`
marca los límites del checkpoint y entrega los diagnósticos al host.

El runner añade `--require-microtasks`, `--gc-between-microtasks` y
`--observer-probe`. La sonda SpiderMonkey solo observa cambios DOM; no evalúa ni
transporta nuestro programa. La regresión de navegación encola un token con el
pipeline ya retirado y comprueba su no ejecución antes de publicar la prueba.
Los informes finales están en `native-promise-proof.json`,
`native-microtask-interop-proof.json` y `native-promise-gc-proof.json`.

## Async/await y Fetch

`mwr_network.rs` conecta callbacks nativos de Fetch y Response con las
continuaciones descritas en [ASYNC.md](../ASYNC.md). Los handlers solo conservan
pipeline y serial; la sesión posee AbortControllers y Responses nativos hasta
que la VM libera sus wrappers. Document.abort cancela transporte con el contexto existente. Document.unload
retira primero la sesión y cancela después, de modo que ningún callback tardío
pueda reactivar una VM descargada.

```sh
python3 gc-lab/servo-native/run_fetch.py --servo gc-lab/.servo-native/target/debug/servoshell
```

El runner sirve HTTP local y espera observaciones DOM nativas, manteniendo
bloqueado el evento load con un contador nativo exclusivo de las pruebas mientras continúa la red. Comprueba
éxito, 404, 204, fallo de conexión, CORS, CSP y navegación con un cuerpo pendiente.
No usa el self-test de click sincrónico para simular una operación de red.
Ver [FETCH.md](../FETCH.md) para límites y estado de la evidencia.

La caché de dependencias se identifica por versión, toolchain y manifiestos,
sin crear una entrada nueva por cada edición del intérprete. El archivo de
fuentes verificado también se guarda antes de compilar, incluso si las pruebas
posteriores fallan. Los cuatro paquetes de integración se siguen limpiando
antes de cada compilación nativa.

## Modulos ES con carga nativa

[MODULES.md](../MODULES.md) describe el backend optativo
`application/x-mwr-module`, inline o con `src`. `run_modules.py` verifica once
escenarios HTTP, incluidos CSP, CORS, MIME, errores y navegación con una fuente
pendiente. Los programas no se compilan en SpiderMonkey. El hito verificado
está en `native-module-proof.json`, con `native-modules.png`. Las capturas usan
bloqueadores de carga nativos exclusivos de las pruebas, sin peticiones de imagen
que puedan retener el documento retirado.
