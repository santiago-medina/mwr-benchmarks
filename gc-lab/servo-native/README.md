# Integración nativa experimental en Servo 0.7.0

Este directorio contiene el adaptador dentro del hilo de script y un parche
mínimo sobre Servo fijado a `aac43a3f31a259f04a574f5ec4e959c943ad7cc7`.
La validación completa requiere COMPILAR el fork y ejecutar `run.py`; la existencia
del parche por sí sola no demuestra integración funcional.

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
parche excluye `vendor` de la autodetección del workspace y registra cinco
paquetes ICU como miembros y dependencias locales. Así cbindgen usa el lockfile
principal al invocar cargo metadata, en vez de los lockfiles antiguos empaquetados
en esas crates. No se modifica código ni checksums de las dependencias vendorizadas.

`MWR_NATIVE_JS=1` habilita un único script inline `application/x-mwr-js` por
documento. El parser HTML entrega el texto directamente a nuestra VM. No hay
WebDriver, proceso de intérprete separado, RPC, eval de transporte ni serialización
de closures. El listener extranjero forma parte del dispatcher DOM de Servo.
Se conservan los controles previos de scripting y CSP de la ruta de preparación.
El estado de VM se suspende entre invocaciones y toma prestado el contexto DOM
solo durante la llamada; el adaptador no fabrica contextos ni usa unsafe.

`MWR_NATIVE_SELF_TEST=1` comprueba el texto inicial, pulsa con
`HTMLElement::Click` desde Rust, verifica «Funciona», suelta las raíces de nuestra
VM y vuelve a pulsar para comprobar que el listener fue retirado. Es un evento
sintético del DOM real, no un clic físico. El runner solo arranca el navegador y
recoge logs y PNG; las operaciones y verificaciones DOM ocurren dentro de Servo.
Los controles adicionales deshabilitan el backend, esperan sin pulsar y prueban
un programa con sintaxis rechazada por nuestro parser.

Límites: SpiderMonkey sigue siendo necesario para reflectores, contexto y GC de
Servo. Los leases `Trusted<Node>` son raíces fuertes explícitas y se retiran al
recoger wrappers o destruir la ventana. Esto no es tracing conjunto de ambos
heaps. Solo hay click sin objeto Event en la VM; no se soportan scripts externos,
varios programas por documento, reentrada de callbacks ni `.click()` desde el
programa experimental. La navegación normal limpia leases/listeners antes de
destruir el runtime; el teardown tras un pánico solo descarta las raíces propias.
No es un motor JS compatible con la web ni un sandbox para páginas no confiables.
