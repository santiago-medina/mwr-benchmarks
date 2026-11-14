# Direct eval y captura de referencias de enlace — diagnóstico

Fecha: 2026-11-14. Snapshot Test262 `e4fbf57745c6aae3d1f12f181ecef78b1665d812`.

## Evidencia reproducida

[GitHub Actions run 43568971786](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/43568971786):
`conformance/audit_binding_eval.py` ejecutó **14 casos oficiales × cuatro
configuraciones = 56 ejecuciones**, en modo sloppy, Tree/VM y con/sin GC stress.
Todos los **56 casos fallaron** (no hay excepciones de runner ni timeouts).
El artefacto `mwr-eval-reference-diagnostics` incluye rutas, motivos y estados
individuales. Este job es intencionadamente diagnóstico: su conclusión verde
significa que completó la matriz, **no** que la suite pasara.

Casos:
- `language/expressions/compound-assignment/S11.13.2_A6.{1..11}_T1.js`
- `language/expressions/assignment/S11.13.1_A6_T{1..3}.js`

Los once casos compuestos usan una forma de:
```js
function f() {
  var x = 3;
  var inner = (function () {
    x *= (eval("var x = 2;"), 4);
    return x;
  })();
}
```
La referencia `x` del lado izquierdo apunta a la declaración externa. El
`eval` del RHS introduce un nuevo `var x` local. `PutValue` debe actualizar
el **registro de entorno resuelto en el momento de evaluar el LHS**, no buscar
el nombre otra vez desde el entorno local después del `eval`.

## Hipótesis de causa raíz confirmada por inspección, pendiente de parche

- `src/script/vm.rs`: `Resolved::Found` es un booleano sin el Id del entorno.
  `resolve_name_hinted` recorre los entornos y descubre el correcto, pero
  descarta su identidad.
- `src/script/references.rs`: `name_reference` genera
  `Reference::Binding(env, name)` con el entorno de *partida*, no el entorno
  donde se halló el enlace. `reference_set` llama a
  `assign_binding(env,...)` y vuelve a resolver tras el RHS.
- `src/script/interp.rs`: la instrucción `ResolveName` codifica
  `Found` como `Value::Bool(true)`, perdiendo la identidad del entorno.
  `GetRef`/`SetRef` reconstituyen el enlace con `reference_of` y parten
  de nuevo de `env`.

**Diseño propuesto:** `Resolved::Found(Id)` debe capturar el registro correcto,
`Reference::Binding` debe preservar tanto ese Id como el flag de strictness
del contexto original, y los tokens de referencia de la VM deben conservar Id,
tipo de enlace (binding/with/unresolvable) y raíces GC. Evitar usar
identificadores nativos no tipados o campos destinados a valores JS para
codificar referencias: sería una optimización frágil. Un tipo interno
explícito de referencia VM es preferible, con pruebas de raíz y recursión.

## Criterios para cerrar

1. Los 14 casos oficiales pasan en Tree y VM, con GC threshold/stress (56/56).
2. No hay regresión de la suite Rust en debug/release, incluida la prueba de
   recursión y liberación de raíces.
3. No aparece ninguna regresión de Test262 full con el binario Temporal y el
   snapshot fijado; documentar conteo de aprobados y cambios de motivos.
4. El caso de `with`, referencias de módulos y propiedades globales debe
   seguir funcionando; añadir regresiones específicas para esos enlaces.

**Estado:** problema reproducido y causa arquitectónica localizada. No está
corregido; ningún conteo de conformidad nuevo se atribuye a esta investigación.
