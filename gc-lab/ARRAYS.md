# Arrays, iteración y lista de tareas

Se añaden arrays dispersos, literales con huecos, constructor `Array`,
`Array.isArray`, `push`, `pop`, `join`, `toString`, `map`, `filter`, `forEach`,
`some`, `every`, `keys`, `values`, `entries` y `for…of`.
La lista `examples/task-list.js` añade nodos `<li>`, completa la primera tarea y
elimina la última. Sus datos son objetos dentro de un array; el render usa
`for…of`. `appendChild` y `removeChild` son operaciones tipadas del adaptador DOM.
El adaptador nativo llama directamente a los métodos de Servo, sin ejecutar
código JavaScript de transporte. Los adaptadores que no implementen estas dos
operaciones devuelven un error explícito; el DOM simulado interno no las ofrece.

## Invariantes para revisión

- `objects.rs` sigue teniendo una sola validación de descriptores, `Data::apply`.
  Es una operación sobre una copia; `replace_data` realiza el commit de aristas.
- Definir un índice actualiza índice y `length` en el mismo commit. El fallo de
  cuota no puede publicar un índice que quede fuera de `length`.
- Al reducir `length`, se eliminan índices en orden descendente. Si un índice no
  configurable bloquea el truncado, se conservan las eliminaciones anteriores,
  se restaura el mínimo `length` permitido y se aplica el `writable:false`
  solicitado. Ese resultado parcial es el comportamiento especificado.
- Un hueco es ausencia de propiedad. `map` conserva huecos; `filter` empaqueta
  resultados. Ambos leen valores heredados y usan una instantánea del tamaño,
  pero consultan las propiedades actuales después de cada callback.
- Como no hay Proxy, se pueden saltar tramos sin propiedades examinando las
  claves del objeto y sus prototipos. Así un array con un único índice lejano no
  consume un millón de pasos solo para recorrer huecos. El índice siguiente se
  vuelve a buscar después de cada callback, respetando mutaciones.
- `Key::String` y `Key::Symbol` son variantes distintas; no se usan cadenas
  reservadas para fingir símbolos. La reflexión ordena índices, cadenas y símbolos.
- El iterador conserva una arista al array y consulta su tamaño en cada `next`.
  Al terminar elimina esa arista y permanece terminado aunque el array crezca.
- `for…of` obtiene y conserva `next` una sola vez, crea bindings léxicos nuevos
  por iteración y ejecuta `IteratorClose` al salir del cuerpo por `break`, `return`
  o excepción. `continue` no cierra el iterador. Un error de `next` tampoco provoca
  un segundo cierre. Una excepción pendiente prevalece si `return` falla.
- Valores pendientes de `return` o `throw` se protegen durante un cierre que
  pueda asignar memoria. Los tests incluyen arrays cuyo único dueño es el iterador.

## Medición oficial

Los dos perfiles usan el mismo snapshot verificado de Test262 y el mismo runner.
No se modifica el cuerpo de los tests. Se mantienen visibles los helpers,
modos de ejecución, sintaxis e intrinsics todavía no soportados.

| Perfil | Casos | Aprobados | No soportados | Fallidos |
| --- | ---: | ---: | ---: | ---: |
| Objetos | 2.837 | 1.421 | 1.372 | 44 |
| Arrays e iteración | 2.110 | 1.010 | 1.100 | 0 |

Los 1.185 aprobados del hito anterior se conservan. Se amplía explícitamente esa
referencia a 1.421. Los 44 fallos de objetos son propiedades estándar ausentes:
32 anteriores y 12 métodos de Array que ahora pueden examinarse al existir Array.
El fallo anterior del descriptor de `parseInt` se resolvió al implementar la
función para los callbacks de arrays; no se añadió un stub para su descriptor.
Los 2.431 casos aprobados también pasan en Node 24 con `sta.js` y `assert.js`
oficiales, sin modificar. No es una certificación de compatibilidad completa.

```sh
python3 gc-lab/conformance/fetch.py
cargo build --manifest-path gc-lab/Cargo.toml --bin js_check --release --offline
python3 gc-lab/conformance/run.py --source gc-lab/.test262-source --check-baseline
python3 gc-lab/conformance/run.py --source gc-lab/.test262-source --profile arrays --check-baseline
python3 gc-lab/conformance/compare_node.py --source gc-lab/.test262-source --profile arrays
```

El módulo `arrays.rs` concentra las operaciones exóticas, los métodos y el estado
mínimo del iterador; reutiliza el GC y los descriptores existentes. La suite tiene
111 contratos, incluidos fallos de cuota con realm ya inicializada y liberación
de nodos al sacar una tarea del array. La regresión nativa conserva 1.500 ciclos y
dos navegaciones; el único camino al nodo capturado pasa ahora por un array y un
objeto. La CI registra el resultado de esa ejecución por separado de los contratos.

## Límites explícitos

No se implementa toda la biblioteca Array: faltan, entre otros, `sort`, `splice`,
`slice`, `concat` y `reduce`. No hay spread, destructuring, clases, Proxy ni
iteración asíncrona. Los símbolos soportados cubren identidad, claves y los
well-known `iterator`, `species` y `toStringTag`; descripciones y boxing de símbolos
siguen fuera del subconjunto. `for…of` admite declaraciones simples de un binding,
sin etiquetas ni destructuring. El parser no implementa todas las early errors.
Los fallos de memoria y límites de ejecución no se convierten en excepciones JS:
ni `IteratorClose` ni un `return` de `finally` pueden ocultarlos. Se prueban con
cuotas barridas después de inicializar la realm. Las cuotas siguen siendo contabilidad lógica del grafo, no un límite de RSS.

Cada evento externo recibe un presupuesto de ejecución nuevo. Un `.click()`
programático o reentrante conserva el presupuesto de su llamador; no puede
reiniciarlo para eludir el límite. Se comprueba una secuencia de 50 eventos.
