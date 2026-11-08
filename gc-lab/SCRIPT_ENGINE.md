# Primera ejecución vertical de JavaScript

Este documento conserva el primer hito. Para el estado posterior, ver [OBJECTS.md](OBJECTS.md), [ARRAYS.md](ARRAYS.md), [PROMISES.md](PROMISES.md), [ASYNC.md](ASYNC.md) y [FETCH.md](FETCH.md).

Esta página describe la etapa inicial con DOM simulado. La extensión posterior
a DOM real de Servo está documentada en [SERVO_INTEGRATION.md](SERVO_INTEGRATION.md).

## Resultado y reproducción

El programa de `examples/button.js` se analiza y ejecuta: crea un nodo, registra
una closure JavaScript, dispara click y escribe «Funciona». No se reconoce el
texto del ejemplo para sustituirlo por acciones predeterminadas. El parser
produce un AST que el intérprete recorre, y se prueban programas diferentes.

```sh
cargo run --bin script_demo --release --offline -- examples/button.js
```

Salida esperada:

```json
{"ok":true,"value":"Funciona","live_before_release":3,"live_after_release":0,"metadata_after_release":0,"roots_after_release":0}
```

Los tres objetos supervivientes antes de soltar el realm son el entorno global,
el nodo DOM y la closure del listener. El nodo conserva la closure; la closure
conserva el entorno; el entorno conserva el nodo. Al retirar la raíz global, el
coordinador de ambos heaps recupera el ciclo entero. Una raíz externa opcional
sobre el nodo lo conserva y al liberarla se recupera el ciclo.

## Componentes y contratos de revisión

| Archivo | Responsabilidad |
| --- | --- |
| src/script/parser.rs | Tokens, AST, límites y comprobaciones léxicas tempranas |
| src/script/vm.rs | Valores, entornos, closures, llamadas y raíces temporales |
| src/script/host.rs | Adaptador DOM simulado y despacho síncrono de eventos |
| src/bin/script_demo.rs | Entrada por archivo, salida JSON y liberación comprobable |
| compare_script.py | Comparación con Node y anfitrión simulado declarado |

Los entornos y closures son objetos del heap JS. No son closures de Rust que
capturen estado fuera del trazado. Los nodos están en el heap DOM. Los metadatos
de AST, bindings y texto viven en estructuras Rust indexadas por Id; no son raíces.
Engine::collect poda metadatos cuyos IDs hayan muerto.

Relaciones visibles al GC:

* Entorno -> entorno léxico padre.
* Entorno -> objetos que inicializan sus bindings.
* Closure -> entorno capturado, compartido y no copiado por valor.
* Nodo -> closures registradas como listeners.
* Valor nativo enlazado -> nodo receptor, cuando se conserva en un binding.

Cada evaluación protege callee, receptor y argumentos antes de evaluar algo que
pueda asignar. Los retornos objeto se entregan como temporales: se enlazan o
protegen antes del siguiente safepoint. Los marcos de ejecución liberan sus raíces
con Drop; los errores se propagan como Result y los errores de ejecución liberan
el realm actual. Las comprobaciones de sintaxis se completan antes de cambiarlo;
por eso un error de parseo conserva un realm previo válido.

## Subconjunto explícito

Se soportan declaraciones const, identificadores ASCII, strings con Unicode en su
contenido y escapes básicos, acceso por punto, llamadas, asignación a miembros,
funciones flecha con parámetros y cuerpo de bloque, return con expresión y
comentarios. Se requieren puntos y coma: no hay ASI. Return seguido de salto de
línea y salto de línea antes de => se rechazan. Se comprueban declaraciones
léxicas duplicadas y colisiones parámetro/const también en funciones no invocadas.
Los errores estructurales usan offset cero cuando el AST aún no conserva spans.

No hay números, coerciones ECMAScript completas, arrays, objetos literales,
operadores, let/var, reasignación de variables, constructores, this, eval, módulos,
promesas, async ni WebAssembly. No hay bytecode ni JIT: es un intérprete de AST.
Las limitaciones se rechazan; esto no es un motor conforme a ECMAScript.

El adaptador incluye document.createElement, textContent, tagName, click,
addEventListener y removeEventListener únicamente para click y closures del
intérprete. No hay DOM real, rendering, captura/burbujeo ni Event. Los métodos
nativos se representan como métodos enlazados: extraerlos conserva el receptor;
no se implementan las reglas generales de this de JavaScript. Los errores del
listener se propagan sincrónicamente: es la política del mock, no la del navegador.
El despacho toma un snapshot: ignora nuevos listeners hasta otro click, respeta
la retirada antes de su turno y evita registros duplicados.

## Recursos y limitaciones

GC en modo estrés se ejecuta antes de cada asignación administrada y después de
cada sentencia; también puede ocurrir al agotar la cuota. Se prueban las cuatro
combinaciones de Stable/Indirect de los dos heaps, con estrés activado/desactivado
para el ejemplo completo. Hay límites de fuente (8 MiB), tokens (1 Mi), nesting
sintáctico, llamadas (32), reentrada nativa (32) y pasos (10000 por defecto).

El presupuesto de Runtime sigue siendo de bytes lógicos. Los 64 bytes de payload
reservados por objeto del intérprete son una carga de laboratorio: NO contabilizan
el tamaño real de Strings, HashMap, clones del AST y todos los metadatos. No se
promete un límite duro de memoria ni recuperación completa ante OOM. Las reservas
Rust externas y el parser no están bajo una cuota global.

No hay auditoría de seguridad, sandbox, Miri ni garantía ante código hostil. El
recolector sigue siendo stop-the-world y monohilo. El uso de Drop no implica
limpieza con panic=abort o terminación externa. Los contadores live_after_release
miden objetos lógicos/metadatos/raíces, no RSS ni capacidad retenida de contenedores.

## Validación

La suite completa tiene 46 tests de integración (30 anteriores + 16 nuevos):

* 5 de parser: AST, programas alternativos, strings, entradas rechazadas, límites
  y errores tempranos en closures no invocadas.
* 5 de ejecución: captura tras retornar, shadowing léxico, temporales entre
  argumentos asignadores, recursión acotada y limpieza/reintento tras errores.
* 6 de DOM: programa pedido en ocho configuraciones, nodos retornados y capturados,
  snapshot de listeners, retirada propia y reentrada, errores y cuotas/pasos.

```sh
cargo test --offline
cargo test --release --offline
cargo build --bin script_demo --release --offline
python3 compare_script.py --binary target/release/script_demo
```

El corpus diferencial incluye diez programas y comprueba éxito/error y valor final
frente a Node (V8), además de cero objetos, metadatos y raíces al finalizar nuestro
intérprete. Node usa un DOM mock independiente en JavaScript con la misma política
síncrona de eventos. Por tanto, contrasta el subconjunto del lenguaje y ese contrato
anfitrión, NO certifica compatibilidad con un DOM real ni con Chrome. Tampoco mide
velocidad o conformidad Test262. Resultados en script-differential.json.

## Siguiente frontera

Antes de ampliar sintaxis: interfaces de tracing para objetos anfitriones externos,
metadatos/strings realmente contabilizados y un corpus semántico más amplio. Este
prototipo demuestra la ruta fuente -> AST -> closure -> evento -> DOM -> GC, pero
no demuestra todavía que pueda sustituir el motor JS de Servo.
