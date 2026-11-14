# MWR JavaScript engine — estado verificable y prioridades

> Auditoría documental: 2026-11-14. Referencia inmóvil: `74cad3d318ca74ae424d063a4e7f0f71b2f73a86` (rama `codex/gc-host-boundary`).
> Este informe se obtuvo leyendo código, configuración e informes versionados. **No se compiló ni ejecutó la suite durante esta auditoría**. Los porcentajes son resultados históricos declarados, no una verificación nueva.

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
