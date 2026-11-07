# Objetos ordinarios y aplicación con estado

El motor ejecuta objetos ordinarios, propiedades de datos y accessors, prototipos,
funciones normales y flecha, constructores, `this` estricto y `try/catch/finally`.
Las operaciones reflejadas soportadas se miden con el [perfil Test262](conformance/README.md).

La aplicación `examples/object-counter.js` mantiene el contador en un objeto.
`count` es una propiedad con getter y setter; el setter rechaza valores negativos
con `RangeError`. El listener es una función normal: recibe el botón como `this`,
actualiza el modelo y escribe el label en el DOM. El HTML nativo está en
`servo-native/object-fixture.html` y contiene ese mismo programa.

## Ruta de revisión humana

1. `src/script/objects.rs`: una representación de descriptor y las operaciones
   `get`, `set`, `define`, `delete` y cambio de prototipo. Las funciones y los
   builtins reutilizan el mismo `Data`; no tienen otro algoritmo de propiedades.
2. `replace_data`: añade todas las referencias nuevas antes de retirar las viejas;
   si falla una reserva, revierte las referencias añadidas sin mutar los metadatos.
3. `src/script/execute.rs`: raíces de temporales, receptores de llamadas y
   completions. Un `return` o `throw` pendiente se protege antes de ejecutar
   `finally`. Un `finally` abrupto sustituye la completion anterior.
4. `src/script/vm.rs`: ámbitos, TDZ, registros globales y liberación de frames.
   Los metadatos no son raíces; prototipos, accessors, valores capturados y
   argumentos enlazados deben tener aristas en el grafo del runtime.
5. `src/script/builtins.rs`: tabla explícita de intrinsics y dispatch hacia las
   operaciones anteriores. `host.rs` conecta los receptores con el adaptador DOM.

En este hito, la prueba de contratos tenía 88 casos en debug y release. Los casos nuevos
recorren heaps estables e indirectos con GC antes de cada asignación de objeto.
También se prueban cuotas, excepciones de getters/setters, error de escritura
nativa capturable y recuperación sin alterar el DOM. El corpus diferencial tiene
19 casos contra Node 24, usando modo estricto y un DOM mock declarado.

## Límites actuales

El hito posterior [ARRAYS.md](ARRAYS.md) añade arrays, símbolos mínimos y `for…of`. Siguen faltando Proxy, módulos, promesas y el lenguaje completo.
El parser admite un subconjunto explícito y tiene límites de tamaño/profundidad;
no es una implementación completa de las early errors de ECMAScript. Los errores
del host durante operaciones DOM son excepciones JS; los fallos de memoria,
cuotas, límites de ejecución y errores de liberación del adaptador son errores
externos que `catch` no absorbe. La cuota del laboratorio contabiliza registros
y aristas del GC, no toda la memoria Rust de cadenas, AST y metadatos.

SpiderMonkey sigue gestionando los reflectores y el GC nativo de Servo. El
programa experimental y sus callbacks se ejecutan en nuestro intérprete Rust.
La prueba nativa usa un click sintético de `HTMLElement::Click`, no entrada física.
El resultado de la aplicación y del GC nativo se verifica separadamente en CI.

## Evidencia de ejecución

El hito nativo guarda la prueba de la aplicación en `native-object-proof.json`
y la del GC real en `native-object-gc-proof.json`, con commit y workflow concretos.
La captura `native-object-counter.png` permite revisar el resultado renderizado.
`conformance/results/summary.json`, `conformance/node-report.json` y
`conformance/differential-report.json` recogen las comprobaciones del lenguaje.

Validación final: [CI 42388366054](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/42388366054),
commit `676483c75cfb92c03600b1d810ed108e46889b88`. Pasaron los cuatro jobs:
contratos, Test262, puente DOM y Servo nativo. El contador pasó de «Cuenta: 0»
a «Cuenta: 1». La regresión conservó 74 objetos de VM de la realm, finalizó
3.000 botones nativos en 1.500 ciclos y verificó dos navegaciones, sin
`release_globals`. La compilación nativa reutilizó la caché y tardó 70 segundos.
