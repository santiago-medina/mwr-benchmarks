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
