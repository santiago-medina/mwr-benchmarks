# Perfil reproducible de objetos ordinarios

Snapshot oficial de Test262: `e4fbf57745c6aae3d1f12f181ecef78b1665d812`.
El SHA-256 del archivo descargado se comprueba en `fetch.py`.

Resultado actual: **2.837 archivos, 1.421 aprobados, 1.372 no soportados y 44 fallidos**.
Se ejecutaron 2.918 aserciones nativas. Esto no es un porcentaje de compatibilidad
con ECMAScript completo. Se recorren íntegramente los 17 directorios declarados
en `run.py`; no se seleccionan únicamente archivos que pasan.

```sh
cargo build --manifest-path gc-lab/Cargo.toml --bin js_check --release --offline
python3 gc-lab/conformance/fetch.py
python3 gc-lab/conformance/run.py --source gc-lab/.test262-source
# El comando anterior devuelve error mientras existan casos fallidos.
python3 gc-lab/conformance/run.py --source gc-lab/.test262-source --check-baseline
```

Cada archivo se ejecuta en un proceso nuevo, añadiendo únicamente el prefijo
estándar `"use strict";`. El cuerpo original permanece intacto. Se portan `assert`,
`sameValue`, `notSameValue`, `throws`, `$ERROR` y `Test262Error` a funciones nativas;
no se carga `assert.js` en nuestro parser. `assert` exige el booleano `true`;
`throws` comprueba la identidad exacta de `.constructor`, no solo el nombre.
No se transforman los includes: los casos que requieren cualquier helper adicional
se registran como no soportados, incluidos `propertyHelper.js` y sus verificaciones.
También quedan fuera los modos sloppy, módulos, async, raw y negativos de parseo.
Sintaxis o intrinsics explícitamente no implementados se registran como no soportados;
una aserción incorrecta, una excepción inesperada o una fuga de GC son fallos.

Los 44 fallos actuales pertenecen a `Object.getOwnPropertyDescriptor`: faltan
propiedades estándar de funciones globales, métodos de `String` y `Number`,
`Function.prototype.apply`, `Object.prototype.toLocaleString` y
`Error.prototype.toString`, además de doce métodos de Array aún ausentes. No se han añadido funciones vacías para aprobar
únicamente sus descriptores. Los casos concretos figuran en `known-failures.json`
y los errores en `results/summary.json`.

La CI usa `--check-baseline`: exige conservar todos los 1.421 aprobados y rechaza
nuevos casos fallidos. **No exige cero fallos conocidos.** El modo normal sigue
fallando con cualquiera de los 44. `--record-baseline` es una operación explícita
para registrar una nueva referencia, no una actualización automática de CI.

El informe completo `results/cases.json` contiene el hash de cada fuente y su
resultado; CI lo conserva como artifact. El resumen añade hashes de implementación
y runner. Tanto las rutas aprobadas como los fallos conocidos se versionan.

Como comprobación independiente, `compare_node.py` ejecuta los 1.421 casos
aprobados en Node 24 con `sta.js` y `assert.js` oficiales, sin modificar.
Los 1.421 pasan también allí; el informe está en `node-report.json`.
Esto valida el oráculo de esos casos, no cubre los 1.372 no soportados.

## Perfil de arrays e iteración

`--profile arrays` recorre íntegramente los directorios y el glob de constructores
indicados en `ARRAY_SCOPES`: 2.110 casos, 1.010 aprobados, 1.100 no soportados y
cero fallos. La referencia se guarda en `required-pass-arrays.json`; los informes
están en `results-arrays/` y `node-report-arrays.json`. El perfil de objetos
conserva todos los 1.185 aprobados anteriores y añade 236. Su lista de fallos
crece al poder ejecutar casos de métodos de Array antes no soportados; ese cambio
se revisó explícitamente. Ver [ARRAYS.md](../ARRAYS.md) para límites e invariantes.
