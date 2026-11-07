# Promesas, jobs y checkpoints compartidos con Servo

El motor incorpora `Promise`, `resolve`, `reject`, `then`, `catch`, `finally` y
`queueMicrotask`. Los executors se invocan sincrónicamente; las reacciones y las
llamadas a thenables son jobs diferidos. Un executor que falla rechaza la promesa;
un error de un handler rechaza la promesa devuelta por `then`. Los errores de
memoria, ejecución o funciones no implementadas siguen siendo terminales, no
rechazos capturables por JavaScript.

## Diseño para revisión humana

- `promises.rs` concentra estados, capabilities, celdas de resolución, funciones
  internas y jobs. Las celdas compartidas garantizan que resolver y rechazar el
  mismo executor solo tenga efecto una vez. Resolver con la propia promesa
  produce un `TypeError`.
- Los slots usan el mismo `Data`, las propiedades ordinarias y el commit
  transaccional de aristas de `objects.rs`. No hay otro recolector para promesas.
- Cada entrada de cola tiene una raíz explícita y un serial monotónico. El job
  contiene aristas a handler, argumento, thenable y capability. Al ejecutarlo,
  una raíz de frame sustituye a la raíz de cola antes de retirarla.
- `Engine.execute` no realiza el checkpoint. Un host independiente llama a
  `microtask_checkpoint(limit)` al terminar la invocación exterior. Servo usa
  `run_one_microtask(serial)` por cada job alojado en su propia cola.
- `NativeAdapter.queue_microtask` encola un `MicrotaskRunnable` real de Servo.
  Guarda únicamente `PipelineId` y serial, sin `Document`, `Trusted<Node>` ni
  punteros de VM. Por ello una entrada antigua no mantiene viva una realm.
- Los handlers entran en la realm y en `run_a_script`; el estado se suspende
  antes del checkpoint. Cada job vuelve al planificador compartido, por lo que
  las microtareas nuevas respetan el FIFO global, también con SpiderMonkey.
- La cola tiene un límite de entradas; cada checkpoint tiene además un límite
  de jobs, compartido entre jobs Rust durante el checkpoint de Servo. Los
  errores terminales cancelan las entradas de VM restantes. La reentrada no
  reinicia esos límites.
- `release_globals` cancela jobs y diagnósticos. La navegación elimina la sesión;
  una entrada nativa con su antiguo pipeline resulta un no-op. Las promesas
  pendientes y sus reacciones desaparecen con el grafo de la realm.
- Los diagnósticos de rechazo guardan IDs débiles y texto, no raíces a promesas.
  Se retiran si se añade un handler y se entregan al host al finalizar el
  checkpoint. Los fallos de aserción se conservan aunque el GC ya haya recogido
  la promesa, para evitar aprobados falsos en tests asíncronos.

La suite reúne 128 contratos, con casos de FIFO, thenable getters, exactamente
una resolución, especies, autorresolución, `finally`, cuotas, cancelación y un
nodo extranjero cuyo único dueño es un job. Un error en `queueMicrotask` se
notifica al host y no impide ejecutar las entradas posteriores.

## Perfil oficial

Se usa el snapshot fijado de Test262 y se recorren los globs/directorios
`PROMISE_SCOPES`, sin transformar el cuerpo de los casos. Este perfil permite
`flags: [async]`: exige exactamente un `$DONE`, sin error, y vacía la cola de jobs
con un límite. Los includes adicionales permanecen no soportados; no se crean
versiones parciales de sus helpers. Syntax/Unsupported se clasifican antes de
exigir `$DONE`. Un rechazo no manejado de `Test262Error` impide aprobar el caso.

| Perfil | Casos | Aprobados | No soportados | Fallidos |
| --- | ---: | ---: | ---: | ---: |
| Objetos | 2.837 | 1.424 | 1.371 | 42 |
| Arrays | 2.110 | 1.010 | 1.100 | 0 |
| Promesas | 221 | 125 | 96 | 0 |

Se conservan todos los aprobados anteriores. Los 125 del perfil de promesas
pasan también en Node con `sta.js` y `assert.js` oficiales y un checkpoint real
antes de comprobar `$DONE`. `Reflect.construct`, `Reflect.apply`,
`Function.prototype.apply` e `Array.prototype.indexOf` tienen implementaciones
reales, necesarias para probar constructores, capabilities y orden de propiedades;
no se añadieron funciones vacías para aprobar descriptores.

```sh
cargo build --manifest-path gc-lab/Cargo.toml --bin js_check --release --offline
python3 gc-lab/conformance/run.py --source gc-lab/.test262-source --profile promises --check-baseline
python3 gc-lab/conformance/compare_node.py --source gc-lab/.test262-source --profile promises
```

## Pruebas nativas verificadas

`promise-fixture.html` contiene el programa de `examples/promise-task.js`.
Cada click resuelve un thenable, encadena un resultado, espera un `finally` y
escribe el orden observado. El payload es un nodo desconectado, retenido por los
jobs. La opción `--gc-between-microtasks` fuerza GC conjunto antes de cada job.

`microtask-interop-fixture.html` comprueba `R → S → U`: un job Rust escribe `R`,
lo que encola un `MutationObserver` de SpiderMonkey; ese observador escribe `S`
antes de que el job Rust siguiente escriba `U`. El observador es una sonda,
no transporte ni evaluación de nuestro programa. La prueba distingue una cola
compartida de vaciar todos los jobs Rust de golpe.

La regresión de 1.500 ciclos conserva además una promesa pendiente entre
invocaciones y realiza dos navegaciones. Después de cada retirada de realm se
encola un token con su pipeline anterior y se comprueba que realmente se ignore.
Los informes y capturas se conservan en el repositorio, con su commit y workflow.

## Límites

No hay `async/await`, temporizadores, `fetch`, módulos ni agentes compartidos.
`Promise.all`, `race`, `allSettled`, `any`, `withResolvers` y `try` permanecen
explícitamente no implementados. No se implementan clases, Proxy ni todos los
casos de `Reflect.construct` con un `newTarget` distinto para otros builtins.
Los rechazos no manejados se notifican como diagnósticos del host; todavía no
hay eventos HTML `unhandledrejection`/`rejectionhandled`. Los límites de memoria
siguen siendo contabilidad lógica del grafo, no RSS. No se afirma compatibilidad
completa con ECMAScript ni sustitución de los reflectores o el GC de SpiderMonkey.

## Evidencia final

[CI 42398228306](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/42398228306)
valida el runtime de `af0a8c297dd8a5dad0b8dbdf3816234a4660deb8`; terminaron
correctamente los cuatro jobs, incluidos 128 contratos en debug/release y los
tres perfiles Test262 con su comprobación independiente en Node.

- `native-promise-proof.json`: dos clicks, 18 jobs y 18 colecciones conjuntas
  antes de sus ejecuciones; resultado `Resultado 4 | adoptar > micro > then > finally`.
- `native-microtask-interop-proof.json`: dos jobs Rust, GC antes de ambos y el
  observador de SpiderMonkey entre ellos; resultado `RSU`.
- `native-promise-gc-proof.json`: 1.500 ciclos, 3.000 botones finalizados, 89
  errores recuperados, baseline constante de 117 objetos, dos navegaciones y
  dos entradas dirigidas a realms retiradas realmente ignoradas.
- `native-promise.png` y `native-microtask-interop.png`: capturas de ambos resultados.

Los tres informes corresponden al mismo binario. El hash de todas las fuentes
Rust actuales coincide con el medido por la prueba nativa. La caché de dependencias
se restauró y la compilación nativa tardó 71 segundos.
