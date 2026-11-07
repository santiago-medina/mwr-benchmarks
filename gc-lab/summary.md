# Resultados del laboratorio

Medianas de nueve ejecuciones por combinación. MB decimales; capacidad estimada, no RSS.

| Carga | Modelo | Capacidad MB | Tabla MB | GC µs | Lecturas µs |
| --- | --- | ---: | ---: | ---: | ---: |
| sparse | Stable | 8.955 | 0.000 | 285 | 365 |
| sparse | Compact | 2.095 | 0.000 | 442 | 354 |
| sparse | Indirect | 5.241 | 2.097 | 344 | 435 |
| dense | Stable | 21.134 | 0.000 | 1066 | 3668 |
| dense | Compact | 19.643 | 0.000 | 2898 | 3697 |
| dense | Indirect | 21.740 | 2.097 | 2248 | 4497 |

Lecturas: 200.000 en sparse, 2.000.000 en dense. Los tokens se resuelven antes del tramo medido.

## Tamaño del código

| Archivo | Líneas con comentarios y vacías |
| --- | ---: |
| src/lib.rs | 406 |
| src/main.rs | 48 |
| src/operations.rs | 54 |
| tests/contracts.rs | 175 |
| tests/scopes_and_map.rs | 213 |

No se ha medido tiempo de revisión humana. Ver README.md para contratos, límites y metodología.
