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
| src/dual.rs | 345 |
| src/host.rs | 71 |
| src/lib.rs | 461 |
| src/main.rs | 48 |
| src/operations.rs | 54 |
| tests/contracts.rs | 175 |
| tests/dual_heap.rs | 303 |
| tests/host_boundary.rs | 103 |
| tests/scopes_and_map.rs | 213 |

No se ha medido tiempo de revisión humana. Ver README.md para contratos, límites y metodología.
