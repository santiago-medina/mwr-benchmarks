# Resultados del laboratorio

Medianas de nueve ejecuciones por combinación. MB decimales; capacidad estimada, no RSS.

| Carga | Modelo | Capacidad MB | Tabla MB | GC µs | Lecturas µs |
| --- | --- | ---: | ---: | ---: | ---: |
| sparse | Stable | 8.955 | 0.000 | 316 | 382 |
| sparse | Compact | 2.095 | 0.000 | 464 | 344 |
| sparse | Indirect | 5.241 | 2.097 | 359 | 451 |
| dense | Stable | 21.134 | 0.000 | 1135 | 3670 |
| dense | Compact | 19.643 | 0.000 | 3086 | 3674 |
| dense | Indirect | 21.740 | 2.097 | 2435 | 4549 |

Lecturas: 200.000 en sparse, 2.000.000 en dense. Los tokens se resuelven antes del tramo medido.

## Tamaño del código

| Archivo | Líneas con comentarios y vacías |
| --- | ---: |
| src/bin/dual_demo.rs | 55 |
| src/bin/script_demo.rs | 50 |
| src/bin/servo_bridge.rs | 72 |
| src/dual.rs | 355 |
| src/host.rs | 71 |
| src/lib.rs | 463 |
| src/main.rs | 48 |
| src/operations.rs | 54 |
| src/script/adapter.rs | 11 |
| src/script/host.rs | 294 |
| src/script/mod.rs | 10 |
| src/script/parser.rs | 428 |
| src/script/vm.rs | 397 |
| src/script/wire.rs | 97 |
| tests/contracts.rs | 175 |
| tests/dual_heap.rs | 303 |
| tests/external_dom.rs | 189 |
| tests/host_boundary.rs | 103 |
| tests/scopes_and_map.rs | 213 |
| tests/script_dom.rs | 99 |
| tests/script_execution.rs | 66 |
| tests/script_parser.rs | 53 |

No se ha medido tiempo de revisión humana. Ver README.md para contratos, límites y metodología.
