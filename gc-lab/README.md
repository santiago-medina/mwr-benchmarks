# Laboratorio de memoria: tres modelos y revisión humana

Prototipo en Rust, sin dependencias externas, con `#![forbid(unsafe_code)]`.
Incluye experimentos de GC y un intérprete de un subconjunto explícito de JS;
no es un motor compatible con ECMAScript ni una réplica de Boa/Nova. La
[integración nativa en Servo](servo-native/README.md) enlaza el intérprete dentro
del navegador manteniendo SpiderMonkey para el DOM. Los contratos de memoria
siguen siendo el núcleo del laboratorio. El [GC conjunto con Servo](servo-native/JOINT_GC.md)
comprueba la supervivencia y recogida de ciclos reales sin retirar las raíces
globales de la VM.

## Reproducir

```sh
cargo test --offline
cargo test --release --offline
cargo run --release --offline > results.csv
python3 summarize.py
```

En Linux sin `/proc/self/exe`, el wrapper LLD puede fallar: usar
`RUSTFLAGS='-C link-arg=-fuse-ld=bfd'`. Si falta `/tmp`, definir `TMPDIR` a un
directorio escribible existente. Son particularidades del entorno usado.

## Diseño

| Modelo | Identificador público | Almacenamiento | Trabajo extra del GC |
| --- | --- | --- | --- |
| Stable | Estable para supervivientes | Slots con huecos reutilizables | Lista de huecos |
| Compact | Se invalida tras cada GC | Vector compactado | Reescribir raíces y aristas |
| Indirect | Estable para supervivientes | Tabla de IDs y vector compactado | Actualizar tabla |

Los tres comparten marcado iterativo, validación, objetos, raíces y API. Los
objetos contienen un entero y aristas ordenadas. La tabla es un
`Vec<Option<usize>>`; no hemos optimizado su representación. Los tokens son
deliberadamente grandes y comprobados en cada acceso. La columna `token_index`
del objeto se conserva también en los modelos directos para no introducir
representaciones físicas diferentes en esta primera comparación.

Cada token contiene heap, índice y número de identidad. Reutilizar un slot no
resucita un token viejo. El modelo compacto directo renueva TODOS sus tokens,
aunque un objeto no cambie de posición: es un contrato conservador del laboratorio,
no una propiedad necesaria de todos los recolectores compactantes.

En Indirect el índice es lógico. La tabla traduce a la posición física, el GC
actualiza esa tabla y las aristas siguen conteniendo el mismo ID. Cuando un objeto
muere se invalida su entrada y se reutiliza su índice con una identidad distinta.
La tabla y las listas libres conservan capacidad según el máximo histórico.

## Raíces por ámbito

`Heap::scope` presta el heap a `RootScope`. Las raíces creadas mediante
`RootScope::root` se liberan en `Drop`, incluyendo retornos `Err` y desenrollado
por `panic`. Los ámbitos anidados respetan las raíces exteriores.

Los slots de raíces se reutilizan con identidades nuevas. Una prueba repite
10.000 ámbitos conservando un token escapado y verifica que no revive y que
solo se necesitan dos posiciones de raíces.

Límites del contrato:

* Un handle no mantiene vivo su objeto: debe existir una raíz o un camino desde ella.
* `scope` gestiona sus raíces propias, no todas las raíces manuales del heap.
  Su `DerefMut` permite acceder a la API manual; esto favorece experimentar, pero
  NO impone las capacidades de un motor de producción mediante el sistema de tipos.
* Se puede devolver un token de raíz fuera del ámbito; posteriormente da error.
  El compilador no rechaza este escape: la validación es dinámica.
* El resultado handle de `map` es temporal; el llamador debe enraizarlo antes de
  otro punto de recolección.
* `Drop` no se ejecuta con `panic=abort`, `process::exit` ni terminación externa.
* La reentrada es sincrónica, mediante closures Rust. No existe event loop ni async.

## Estrés

Con `set_stress(true)` se recolecta ANTES de CADA asignación de objeto. La nueva
asignación sigue siendo válida al volver al llamador, hasta el siguiente safepoint.
No significa recolectar en cada asignación interna del asignador Rust.

## Operación revisable

`src/operations.rs` implementa dos caminos para la misma transformación de un
vector fijo de aristas con callbacks. No implementa Array.prototype.map de
ECMAScript: no hay proxies, getters, species, arrays dispersos ni mutación del
input durante el callback.

* `map_stable`: sirve para Stable e Indirect sin modificar el algoritmo. Conserva
  handles locales mientras sus objetos permanecen enraizados.
* `map_resolving`: sirve para los tres; vuelve a resolver fuente y destino después
  de puntos donde puede haber recolección.

Ambos usan ámbitos anidados para el item y para el resultado en construcción.
Ambos delegan un callback que puede asignar, recolectar y volver a entrar. El
callback devuelve un entero, no un objeto: no se ha validado la transferencia de
propiedad/raíces de un resultado objeto externo.

Obligaciones de revisión observadas:

| Obligación | Stable | Compact | Indirect |
| --- | --- | --- | --- |
| Mantener objetos alcanzables | Sí | Sí | Sí |
| Limpiar raíces del ámbito | Automático | Automático | Automático |
| Refrescar tokens locales tras GC | No | Sí | No |
| Verificar tabla ID/posición dentro del GC | No | No | Sí |
| Validar IDs caducados/reutilizados | Sí | Sí | Sí |

Eso muestra una diferencia de contrato, NO un tiempo medido de revisión humana.
Para medirlo harían falta revisores y cambios equivalentes con errores sembrados.

## Verificación

Las 20 pruebas iniciales de integración pasan en debug y release:

* Las ocho pruebas anteriores ahora se ejecutan en los tres modelos: ciclos,
  reentrada, referencias caducadas, tokens de otro heap, cadenas de 20.000 objetos,
  errores y 32 grafos generados por modelo con oráculo independiente.
* Siete pruebas nuevas: limpieza por ámbito ante éxito/error/panic, reutilización
  de raíces, estrés en grafos, map frente a oráculo escalar con GC reentrante,
  map que falla o entra en panic después de producir resultados parciales,
  reutilización de IDs indirectos y detección de temporales no enraizados/map vacío.

`test-results.txt` y `test-release-results.txt` conservan los registros.
No hemos usado Miri, sanitizadores ni auditoría externa. No hay pruebas de
concurrencia, OOM real, finalizadores, WeakMap/ephemerons, DOM real ni código máquina.
La ausencia de unsafe en esta crate no constituye auditoría de std ni del compilador.

## Mediciones

Rustc 1.99.1, Linux x86_64. Dos cargas de 100.000 objetos:

* sparse: sobrevive uno de cada diez.
* dense: sobreviven todos.

Cada superviviente tiene una arista al primer objeto vivo. Nueve repeticiones por
modelo/carga, rotando el orden de modelos. Cada ejecución verifica el checksum.
`read_us` mide veinte recorridos de los handles ya resueltos: 200.000 lecturas en
sparse y 2.000.000 en dense. No comparar read_us entre cargas como si fuesen el
mismo número de accesos. `black_box` dificulta eliminar el trabajo al optimizar.

La capacidad retenida incluye slots, tabla, listas libres, raíces y buffers de
aristas según `capacity * size_of`. Excluye temporales del GC, overhead del
asignador, el vector de handles del benchmark y otras estructuras externas.
NO es RSS, memoria máxima ni medición del sistema completo. Compactar construye
un segundo vector temporal; su pico no está medido. Los buffers de aristas no se
compactan con los objetos.

Los resultados exactos y medianas están en `results.csv` y `summary.md`.
No son un benchmark de JavaScript ni una comparación de Boa con Nova.
El entorno es compartido; no se controlaron frecuencia de CPU, temperatura ni
cachés, y nueve repeticiones no permiten conclusiones generales de rendimiento.

Los tamaños cambian respecto al primer experimento porque ahora hay metadatos
adicionales de raíces e IDs. No comparar cifras entre versiones como si el layout
fuese idéntico.

## Decisión

La API de identificadores estables es el candidato preferido para una operación
humana revisable: Stable e Indirect comparten exactamente el mismo map. La tabla
permite compactar sin propagar relocalización al autor de la operación.

La política de compactación queda abierta: con muchos muertos, Indirect ahorra
capacidad respecto a Stable; con todos vivos la tabla cuesta memoria adicional.
En estas cargas el acceso indirecto también tiene coste de tiempo. No conviene
compactar siempre por principio, ni suponer que la indirección sale gratis.

Antes de fijar una arquitectura de producción: integración con DOM real y heaps
separados; presupuesto de bytes y recuperación ante OOM; medición del pico;
entradas adversariales; y revisión humana independiente de las invariantes.
El experimento siguiente debe mantener el contrato sin exponer posiciones físicas.


## Frontera anfitrión/JS simulada

`src/host.rs` añade una representación mínima de nodo y listener mediante tags
enteros. Ambos viven en el MISMO grafo trazado: esto prueba las relaciones de
propiedad y el protocolo de raíces, NO una integración entre dos recolectores ni
el DOM de Servo. No ejecuta JavaScript.

El nodo referencia al listener; el listener captura al nodo. `dispatch` realiza
reentrada sincrónica, recolecta y devuelve un objeto que conserva su captura y el
resultado de la llamada anidada. Su profundidad está limitada a 32 en este demo.
El código es idéntico para los tres modelos gracias a resolver raíces en safepoints.

Cinco pruebas nuevas cubren: resultado objeto que conserva el ciclo hasta perder
la última raíz; listener retirado pero retenido por una raíz externa; agotamiento
después de construir resultados parciales con recuperación y reintento; fallo
parcial al construir el ciclo; y presupuesto cero/raíz del llamador caducada.
Se ejecutan con los tres modelos, con estrés activado en las rutas de éxito y con
estrés tanto activado como desactivado en el fallo de resultados parciales.

`set_object_limit` limita el NÚMERO DE OBJETOS VIVOS y `try_allocate` devuelve
`ObjectLimit` cuando el GC no puede recuperar espacio suficiente. NO limita bytes,
aristas, tablas, stack o asignaciones del sistema. `allocate` sigue siendo una
comodidad infalible para las pruebas antiguas: hace panic si se configura una cuota
y se agota. El módulo host usa exclusivamente `try_allocate` y propaga el error.
Cualquier llamada que pueda recolectar invalida los handles Compact incluso si
termina en error. Las raíces sobreviven mientras su ámbito continúe activo.

Aprendizaje: los ciclos son recuperables si TODAS las referencias relevantes son
visibles para un trazado común. Usar raíces permanentes para cada listener habría
impedido esa recuperación. Llevar esta propiedad a dos heaps reales requiere un
protocolo adicional de tracing o colección de ciclos que este laboratorio no valida.


## Dos heaps separados

La siguiente etapa implementa dos arenas independientes y un coordinador de
trazado, con presupuesto de bytes lógicos y 10 pruebas nuevas (30 en total).
Ver [DUAL_HEAP.md](DUAL_HEAP.md) para invariantes, límites exactos del presupuesto,
evidencia y reproducción. `dual-results.csv` registra los escenarios.


## Primera ejecución de JavaScript

El [intérprete del subconjunto](SCRIPT_ENGINE.md) ejecuta el programa de
`examples/button.js` con closures reales del AST y el DOM simulado. En aquella etapa incluía
46 tests y un corpus diferencial de diez programas contra Node + mock.
Ver SCRIPT_ENGINE.md para reproducción y límites exactos.


## DOM real de Servo

El [puente experimental](SERVO_INTEGRATION.md) ejecuta nuestro intérprete contra
un botón renderizado por Servo 0.7.0, con click nativo y retirada de handles.
SpiderMonkey sigue presente para el transporte DOM; no es un reemplazo interno.
Aquella etapa tenía 52 tests; CI conserva el informe y la captura de la prueba.

## Objetos, estado y ejecución nativa

El estado actual está descrito en [OBJECTS.md](OBJECTS.md): 88 contratos,
19 comparaciones con Node y un perfil de 2.837 casos oficiales de Test262
con aprobados, no soportados y fallidos publicados por separado.
La [integración nativa](servo-native/README.md) ejecuta el contador dentro de
Servo, sin transporte de JavaScript para nuestro programa. SpiderMonkey sigue
siendo necesario para los reflectores y el GC nativo. Las dependencias de la
compilación se reutilizan mediante la caché de Actions.

## Arrays e iteración

[ARRAYS.md](ARRAYS.md) describe el siguiente hito: arrays dispersos, `length`,
métodos, símbolos como claves y cierre de iteradores en `for…of`. Incluye una
lista que añade, completa y elimina nodos reales de Servo, y perfiles de
Test262 separados para objetos y arrays.

## Promesas y microtareas

Esta etapa se describe en [PROMISES.md](PROMISES.md): promesas, capabilities,
thenables y jobs trazados por el GC, con un perfil Test262 asíncrono y conexión
a la cola de microtareas compartida de Servo. Las secciones anteriores documentan
los hitos previos del laboratorio.

## Async/await y red nativa

[ASYNC.md](ASYNC.md) describe las continuaciones explícitas, el trazado de activaciones
y el perfil oficial de 249 casos (40 aprobados, 209 no soportados, cero fallos).
[FETCH.md](FETCH.md) describe las promesas de cabeceras/cuerpo y el adaptador a
Fetch/Response de Servo, con las pruebas HTTP y sus límites. Hay 148 contratos
debug/release; las pruebas de transporte nativo deben comprobarse en sus informes
y en Actions, por separado de los contratos del host simulado.
