# MWR JavaScript engine — estado verificable y prioridades

> Auditoría documental: 2026-11-14. Referencia inmóvil: `74cad3d318ca74ae424d063a4e7f0f71b2f73a86` (rama `codex/gc-host-boundary`).
> El primer inventario se basó solo en código y registros; **las secciones de verificación nueva más abajo incorporan pruebas ejecutadas después en GitHub Actions**. Los resultados históricos conservan su referencia y se distinguen de los resultados actuales.

## Dónde está realmente el motor

- `gc-lab/src/script/parser.rs`: sintaxis y AST.
- `gc-lab/src/script/vm.rs`, `execute.rs`, `loops.rs`: estado `Engine`, semántica y evaluador de AST (tree-walker).
- `gc-lab/src/script/bytecode.rs`: compilador hacia instrucciones de registros.
- `gc-lab/src/script/interp.rs`: intérprete del `Chunk` de bytecode; `EngineMode::{Tree, Vm}`, contador de compilaciones y motivos de fallback. Registros de frames VM en `Vec<Value>` con IDs reflejados en `Runtime::vm_seeds` para el GC.
- `gc-lab/src/dual.rs`: contratos de heap, handles, raíces y recolección.
- `gc-lab/src/script/agents.rs`, `shared_block.rs`, `atomics.rs`: agentes Test262 con heap por agente y bloques compartidos.
- `gc-lab/conformance/run.py`: runner Test262, perfil `full` (`built-ins` y `language`). `js_check` usa VM por defecto según el checkpoint 32; la variante interna `EngineMode::default()` sigue siendo `Tree`.

**Advertencia documental:** las cabeceras de `BYTECODE_VM.md` y `OBJECT_MODEL.md` aún dicen «nothing implemented/no code yet», aunque sus apartados posteriores describen implementación real. `SCRIPT_ENGINE.md` describe explícitamente un primer hito histórico y no debe usarse como inventario de capacidades modernas. `README.md` también es histórico.

## Versión de referencia y conformance

Archivo leído: `gc-lab/conformance/results-full/summary.json` de `codex/gc-host-boundary`.

| Estado | Casos |
|---|---:|
| Passed | 47 458 |
| Failed | 142 |
| Unsupported | 108 |
| Total | 47 708 |

Resultado informado: **99,48 % de casos aprobados en ese corpus/perfil concreto**; no equivale a conformidad completa con ECMAScript. El checkpoint que lo comunica es «test262 agents and shared data blocks» (checkpoint 46, 2026-11-11), identificado como `t262-full53`. Snapshot Test262 fijado por el runner: `e4fbf57745c6aae3d1f12f181ecef78b1665d812`. Los casos se ejecutan individualmente con presupuesto de 10 s; hay opción de reintentar timeouts manteniendo ese presupuesto. Los resultados de pruebas están versionados, pero todavía **no se ha contrastado el hash de implementación del informe con una compilación nueva**.

### 142 fallos — familias relevantes

Agrupación por rutas del informe, **no diagnóstico de causa raíz confirmada**:

| Grupo | Fallos |
|---|---:|
| `language/expressions/assignment` | 12 |
| `language/expressions/compound-assignment` | 11 |
| `built-ins/Promise` | 17 |
| `built-ins/String` | 12 |
| `language/eval-code/direct` | 9 |
| `built-ins/Date` | 9 |
| `built-ins/AsyncFromSyncIteratorPrototype` | 8 |
| `built-ins/Object` | 7 |
| `built-ins/Set` | 5 |

Otros errores de orden de evaluación aparecen en `language/expressions/{bitwise-and,bitwise-or,bitwise-xor,division,exponentiation,left-shift,modulus,multiplication,right-shift,subtraction,unsigned-right-shift}` (uno en cada grupo), así como en `super` (cuatro). Es una **hipótesis de trabajo** que varias rutas compartan un defecto de evaluación de referencias/operadores: verificar contra casos individuales antes de cambiar semántica.

### 108 no soportados — causas informadas más frecuentes

- `syntax: unsupported character`: 20; variante strict: 3.
- `syntax (strict): unsupported property key`: 12.
- `ordinary reflection on host objects`: 11.
- `unsupported module syntax or type`: 11.
- `with in generator or async function`: 10.
- `unpaired UTF-16 string storage`: 9.
- `syntax: binding declaration requires initializer`: 8.
- `ShadowRealm.prototype.importValue`: 6.

Estas cifras son **conteos de razones de los 108 casos**, no necesariamente subsistemas disjuntos por causa subyacente.

## Estado del optimizador y rendimiento

- La VM de registros existe, compila scripts y funciones y puede volver al tree-walker para construcciones no compilables. No confundir un fallback con una característica no implementada.
- La documentación de checkpoints registra optimizaciones reales: in-place object slots, omisión de entornos en ciertas llamadas compiladas, sugerencias de lookup global y elementos densos.
- Las cifras de Callgrind son históricas y corresponden a cargas específicas; no generalizarlas como mejora global.
- `experiment/safe-rust-jit` es **otra rama sin `gc-lab`**. Su README se presenta como experimento de coste de frontera entre proceso nativo e hijo y como prueba KVM en C; **no es evidencia de un JIT JavaScript integrado**.

## Plan de ejecución recomendado

1. **Reproducir y fijar la línea base** sobre el commit de referencia, guardando `rustc --version`, modo de motor, hash del binario y archivos de resultados. Construir `js_check` en release; descargar el snapshot fijado mediante `conformance/fetch.py`. No sobreescribir el artefacto histórico: usar `--output` separado.
2. **Primera familia a depurar: orden de evaluación y referencias.** Elegir casos `target-member-computed-reference.js`, `target-super-computed-reference.js`, `language/expressions/{bitwise-and,division}/order-of-evaluation.js`; reproducir en `Tree` y `Vm`, inspeccionar `references.rs`, `execute.rs`, `interp.rs`, `bytecode.rs`. Si solo falla VM, priorizar lowering; si fallan ambos, priorizar operación semántica compartida. Agregar regresiones mínimas antes de corregir.
3. **Segunda familia: Promise/AsyncFromSyncIterator.** Distinguir IteratorClose, orden de Get/Call, thenables y rechazo; una corrección abstracta podría cubrir casos de ambas familias, pero esto aún no está demostrado.
4. **Tercera: strings y UTF-16**; separar las 9 no soportadas por representación interna de los 12 fallos `String` y verificar la relación.
5. Después abordar grupos pequeños y bien delimitados: `Array.prototype.indexOf/length-zero-returns-minus-one.js`, descriptores, `Map`/`Set` species, etc.
6. **Documentación:** añadir un estado actual en el README principal de `gc-lab` que enlace esta hoja de ruta. Marcar claramente como **históricos** `SCRIPT_ENGINE.md` y las secciones de diseño inicial de `BYTECODE_VM.md`/`OBJECT_MODEL.md`; no reescribir los checkpoints.

### Comandos propuestos para reproducibilidad (a ejecutar; no ejecutados aquí)

```sh
git checkout 74cad3d318ca74ae424d063a4e7f0f71b2f73a86
cargo build --manifest-path gc-lab/Cargo.toml --bin js_check --release --offline
python3 gc-lab/conformance/fetch.py
python3 gc-lab/conformance/run.py --profile full --source gc-lab/.test262-source \
  --binary gc-lab/target/release/js_check --output gc-lab/conformance/results-audit-full \
  --retry-timeouts 1
```

El runner devuelve error mientras queden fallos; esto es esperado. Antes de utilizar el comando, verificar que el snapshot está descargado y que existe un entorno con recursos suficientes para ejecutar ~47 000 procesos con timeouts de 10 s.

## Límites de lo que se ha verificado

Esta auditoría **no** demuestra que los 142 fallos se reproduzcan ahora, que el motor sea seguro para código hostil, que cumpla el 100 % de la especificación, ni que las pruebas históricas reflejen otro commit. El trabajo siguiente debe producir logs de ejecución nueva, diferencias Tree/VM y pruebas de regresión por causa raíz.

## Verificación nueva mediante GitHub Actions (2026-11-14)

La rama de auditoría ejecutó correctamente el workflow existente `GC laboratory` en el commit `82aeac79c1d27a3f77ccdfe2067c46d0e2484144`: [run 43562134576](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43562134576). El árbol del motor es idéntico al de `74cad3d`: el commit solo añadió esta documentación.

- `cargo fmt --check`: éxito.
- `cargo test --offline`: **191 pruebas aprobadas, 0 fallos**.
- `cargo test --release --offline`: **191 pruebas aprobadas, 0 fallos**.
- `dual_demo`, `script_demo` y corpus diferencial del primer prototipo: éxito.
- Integración con Servo mediante el bridge y DOM/click real: éxito.
- Integración **nativa completa** de Servo (`servo-native`): **omitida** por la condición del workflow; no confundirlo con un pase.
- Perfiles de Test262 ejecutados de nuevo:

| Perfil | Total | Aprobadas | No soportadas | Fallidas | Regresiones del baseline |
|---|---:|---:|---:|---:|---:|
| objects | 2 837 | 2 012 | 825 | 0 | 0 |
| arrays | 2 110 | 1 862 | 248 | 0 | 0 |
| promises | 221 | 171 | 50 | 0 | 0 |
| async | 249 | 128 | 121 | 0 | 0 |
| temporal (segunda crate) | 4 613 | 4 613 | 0 | 0 | 0 |

**Los perfiles se solapan y no deben sumarse como casos independientes.** Los trabajos de comparación con Node (harness, promesas, async y módulos) pasaron también; véase el registro del job `test262-objects`. Estas pruebas seleccionadas utilizan `--check-baseline`, que preserva los aprobados y rechaza nuevos fallos: no equivalen a la ejecución del perfil `full`.

**Perfil completo:** se añadió un workflow separado en `.github/workflows/full-conformance-audit.yml` para ejecutarlo con `--profile full`, guardar los 47 708 resultados individuales y comparar las cifras frente a `results-full/summary.json`. No sustituir la línea histórica `47 458 / 47 708` por una cifra nueva hasta disponer de los resultados de ese workflow. La ejecución de auditoría se inició como [run 43562974007](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43562974007).

**Alcance:** esto verifica los jobs mencionados, no «todos los tests» que existen o podrían ejecutarse en el repositorio. La máquina local de la sesión no dispone de `rustc` y no tiene acceso de red a GitHub; la evidencia reproducible procede de GitHub Actions.

## Auditoría completa reproducida — 2026-11-14

Workflow: [MWR full Test262 audit, run 43563559791](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43563559791), commit `3ccd666849436222aaa0fe26ab162ba8f0c09c5c`. El ejecutable se construyó con `gc-lab/temporal/Cargo.toml` en release: **es necesario usar esta variante para el perfil `full`**, dado que la crate base no habilita `Temporal`.

| Estado | Full53 histórico | Ejecución nueva |
|---|---:|---:|
| Passed | 47 458 | **47 460** |
| Failed | 142 | **140** |
| Unsupported | 108 | **108** |
| Total | 47 708 | **47 708** |

- Snapshot Test262: `e4fbf57745c6aae3d1f12f181ecef78b1665d812` (idéntico).
- No hubo timeouts; `--retry-timeouts 1` no necesitó reintentos.
- La diferencia neta de dos casos coincide con los antiguos fallos de `decodeURI` y `decodeURIComponent` por timeout, que no figuran entre los fallos actuales.
- El workflow conserva `cases.json` y `summary.json` como artifact `mwr-test262-full-audit`; esto permite revisar las **140 rutas fallidas** individualmente.
- **Importante:** la conclusión `success` de ese workflow significa que se ejecutó y se guardó el informe; el runner devuelve un código no cero por los 140 fallos conocidos y el job conserva resultados para análisis. No declarar 100 % de tests aprobados.

### Error de configuración identificado y corregido

La primera ejecución ([run 43562974007](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43562974007)) usó por error el binario sin Temporal, produciendo 42 847 passes, 4 753 fallos y 108 no soportados. **4 167** errores decían `Temporal is not defined`. Esos números no son una regresión del motor y no deben reemplazar el checkpoint. La nueva ejecución usa el binario correcto. Mantener las dos referencias de CI como evidencia de la configuración correcta.

### Verificación de GC/VM y estática

[Run 43563646596](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43563646596): **32/32** escenarios de smoke (`gc-lab/conformance/audit_vm_gc.py`), cruzando ocho programas con `EngineMode::{Tree,Vm}` y GC normal/estrés. [Run 43563769700](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43563769700): la misma matriz **32/32**, tests release de la crate Temporal **13/13**, comprobación de sintaxis Python correcta y `cargo clippy --all-targets` concluido correctamente. Clippy informa **80 advertencias** de biblioteca (82 en el target de pruebas, con duplicados), que requieren triage pero no son errores de compilación. Estos 32 escenarios son un muestreo nuevo, no la suite completa bajo GC stress.

### Pendiente de ejecución independiente

- `servo-native` se omitió en el workflow de CI ordinario: requiere `workflow_dispatch` o commit con `[servo]`, descarga/fuente Servo fijada y compilación extensa. El puente de Servo/DOM sí pasó.
- No se ejecutaron `47 708` casos en cada combinación `Tree/Vm` y GC normal/estrés. El perfil completo documentado se ejecutó en VM con GC por umbral.
- Benchmark de rendimiento: deben conservarse hardware, compilador, `callgrind`/tiempo y binario antes de comparar velocidades. Los benchmarks de checkpoints anteriores son históricos.

**Próximo desarrollo:** abordar las 140 fallidas por causas comunes (orden de evaluación/referencias, Promise/AsyncFromSyncIterator, primitivos de String y Date) y las 108 no soportadas como soporte de sintaxis/host y UTF-16. Cada corrección debe incorporar un repro mínimo y pruebas en Tree, Vm y GC stress, seguido del perfil completo para comprobar no regresiones.

## Correcciones de conformidad y consolidación (2026-11-14)

La rama de auditoría incorporó las correcciones de `indexOf` ([PR #2](https://github.com/santiago-medina/mwr-benchmarks/pull/2), commit de merge `eb078ea`) y del orden de evaluación de asignaciones computadas ([PR #3](https://github.com/santiago-medina/mwr-benchmarks/pull/3), merge `4e33c43`). **Ninguno de estos merges modifica `main` ni `codex/gc-host-boundary`.**

- `indexOf`: para longitud cero, devuelve `-1` antes de convertir `fromIndex`. Caso oficial Test262 aprobado en Tree/VM y GC normal/estrés: [run 43564104029](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43564104029). CI de GC laboratory también aprobada: [run 43564156922](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43564156922).
- Asignación computada: el cálculo de la expresión de la clave precede al RHS, pero `ToPropertyKey` se difiere hasta `PutValue`. En las asignaciones compuestas, `GetValue` y `PutValue` comparten la clave ya convertida para no repetir efectos laterales; la VM emite `PropKey` después de evaluar RHS. Tras correcciones de tamaño de pila y los tests de regresión, pruebas dirigidas en cuatro modos aprobadas: [run 43565348313](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43565348313). CI ordinaria de la rama de cambio (contratos, Test262 por perfiles y bridge Servo) aprobada: [run 43565348356](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43565348356).
- Test262 `full` de **PR #3 aislado**: **47 464/47 708 aprobados, 136 fallos, 108 no soportados**, sin regresiones de la lista aprobada; [run 43565586928](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43565586928). Se observa una mejora de cuatro aprobados respecto a la línea base reproducida de 47 460. **Esta medición aislada no demuestra todavía el número de aprobados tras combinar PR #2 y #3**: el workflow `full` de la rama combinada [run 43566209108](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43566209108) se lanzó para comprobarlo.
- `super[clave]`: otra corrección separada captura `GetSuperBase` antes de convertir una clave computada cuya `toString` puede mutar el prototipo. El PR original #4 pasó Test262 full con 47 464 aprobados y ninguna regresión [run 43565803603](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43565803603), pero quedó en conflicto tras integrar #3; **se cerró sin merge**. Su reemplazo sobre la base combinada es [PR #5](https://github.com/santiago-medina/mwr-benchmarks/pull/5), sujeto a nueva comprobación [Test262 full](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43566343930).

La cifra 47 458 del checkpoint 46 y la referencia reproducida 47 460 siguen siendo referencias históricas importantes. No declarar `full` ni todos los motores/configuraciones conformes al 100 %, y no equiparar un workflow con conclusión `success` (puede conservar fallos conocidos) con 47 708 pruebas aprobadas.

## Línea base integrada de la rama de auditoría (14-11-2026)

**CI reproducida tras fusionar #2 y #3:** [run 43566209108](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43566209108), con el ejecutable `gc-lab/temporal/target/release/js_check` y snapshot Test262 fijado `e4fbf57745c6aae3d1f12f181ecef78b1665d812`. Informe completo y casos individuales guardados en el artifact de esa ejecución.

| Estado | Casos |
|---|---:|
| Passed | **47 465** |
| Failed | **135** |
| Unsupported | **108** |
| Total | **47 708** |

La suite completa ha terminado con cero regresiones de casos anteriormente aprobados, y la CI habitual de la rama integrada también pasó ([run 43566209112](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43566209112)). El porcentaje redondeado es **99,49 %**. Es un aumento neto de siete aprobados respecto al checkpoint 46 histórico (47 458) y cinco respecto a la primera reproducción correcta con Temporal (47 460). No confundir con la cifra aislada de PR #3 (47 464).

### Agrupación de los 135 fallos reales que quedan

Del JSON de casos de esta ejecución, las mayores familias por ruta son: Promise 17; String 12; compound-assignment 11; Date 9; eval-code/direct 9; AsyncFromSyncIteratorPrototype 8; assignment 8; Object 7; Set 5. Las rutas indican dónde fallan las pruebas, **no** necesariamente la causa compartida. Priorizar reproducciones dirigidas antes de cambiar intérprete, bytecode o APIs.

### Correcciones en revisión, aún no incorporadas a esta línea base

- [PR #5](https://github.com/santiago-medina/mwr-benchmarks/pull/5): orden de `GetSuperBase` frente a `ToPropertyKey` en `super[expr]`, reaplicado sobre la rama que contiene #2/#3. Ensayos dirigidos aprobados ([run 43566340935](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43566340935)); full ejecutándose en [run 43566343930](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43566343930).
- [PR #6](https://github.com/santiago-medina/mwr-benchmarks/pull/6): `Set.prototype.union/intersection/symmetricDifference` normaliza `-0` a `+0` en valores almacenados, no solo claves de búsqueda. Los **12 ensayos dirigidos** (tres casos × Tree/VM × GC normal/estrés) aprobaron en [run 43566653063](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43566653063); full ejecutándose en [run 43566663888](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43566663888).

**No sumar mejoras aisladas de distintas ramas** ni actualizar el resultado integrado hasta que sus PR estén fusionados y haya una ejecución completa del árbol combinado.

## Integración verificada de `super` y operaciones de `Set` (2026-11-14)

El [PR #7](https://github.com/santiago-medina/mwr-benchmarks/pull/7) fusionó las dos correcciones sobre la rama de auditoría en el commit `7b987b0e8bc56f306dc458c2849f531187d44669`. Los PR originales #5 y #6 quedaron cerrados **sin merge** por haberse sustituido; el PR #4 también fue reemplazado y cerrado.

- **Test262 completo del árbol combinado:** [run 43567259181](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43567259181), **47 472 aprobados, 128 fallidos, 108 no soportados**, total 47 708; sin regresiones respecto a la lista histórica de aprobados.
- **CI ordinaria completa:** [run 43567236813](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43567236813), contratos debug/release, perfiles Test262, integración Servo/DOM y bridge correctos; la integración nativa completa de Servo permaneció omitida.
- Los casos oficiales dirigidos de [super](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43567253222) (4/4 modos) y [Set](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43567256186) (12/12 modos) pasaron también en la rama combinada.
- Mejora acumulada **+14** aprobados respecto al checkpoint histórico de 47 458; **+7** respecto a la rama integrada previa (47 465). El porcentaje redondeado es **99,51 %**. La suite no es totalmente conforme: quedan 236 casos sin aprobar.

### Nuevos cambios aislados en revisión

El [PR #9](https://github.com/santiago-medina/mwr-benchmarks/pull/9) corrige las consultas indebidas a `@@match`, `@@matchAll` y `@@search` sobre argumentos primitivos. En su rama original (PR #8, cerrado sin merge por divergencia) pasó los **44 ensayos dirigidos** y Test262 completo con **47 476 aprobados, 124 fallidos y 108 no soportados** [run 43567583254](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43567583254); la variante #9 se debe validar contra el motor ya integrado antes de fusionarla.

El [PR #10](https://github.com/santiago-medina/mwr-benchmarks/pull/10) corrige la diferencia entre `String(Symbol())` y `new String(Symbol())`. Su caso oficial pasó las cuatro configuraciones en [run 43568139115](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43568139115); full y contratos del cambio siguen sujetos a verificación.

**Disciplina de evidencia:** no sumar pasadas de ramas independientes ni declarar una cifra nueva integrada sin ejecutar el workflow de Test262 `full` sobre el SHA ya fusionado.
