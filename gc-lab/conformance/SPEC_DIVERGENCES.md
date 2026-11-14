# Discrepancias y falsos positivos potenciales del corpus Test262

Esta nota separa fallos de implementación demostrados de casos cuyo resultado
esperado parece contradecir la especificación vigente. Es **análisis**, no una
lista de excepciones automáticas para ocultar fallos.

## Promise.try y la identidad de la promesa devuelta

Snapshot Test262 fijado: `e4fbf57745c6aae3d1f12f181ecef78b1665d812`.

Casos:
- `built-ins/Promise/try/avoids-wrap.js`
- `built-ins/Promise/try/avoids-wrap-for-subclass.js`

Ambos esperan que `Promise.try(() => sentinel)` devuelva **el mismo objeto**
`sentinel` cuando este es una promesa. La semántica publicada de
[ECMAScript 2026 §27.2.4.8](https://tc39.es/ecma262/2026/multipage/control-abstraction-objects.html#sec-promise.try)
establece primero `NewPromiseCapability(C)` y al final devuelve
`promiseCapability.[[Promise]]`; se llama a la función `resolve` con el valor
del callback, pero **no se retorna directamente ese valor**. Por ello la
igualdad de identidad exigida por estas pruebas parece incompatible con el
texto normativo. El comportamiento actual de MWR en
`gc-lab/src/script/promise_combinators.rs` sigue ese algoritmo, creando la
capacidad, invocando su resolver y devolviendo `cap.promise`.

**Decisión:** no modificar `Promise.try` para devolver la promesa del callback
sin validación normativa adicional. Esta optimización rompería la identidad
del resultado exigida por el algoritmo y los efectos observables del
constructor de la capacidad. Mantener los dos casos como fallos del corpus
hasta corroborar si existen erratas/cambios normativos en TC39/Test262.

**Comprobación independiente:** `.github/workflows/promise-try-spec.yml`
verifica con Node.js 24 el contrato de identidad de `Promise.try` y su
subclase, sin alterar la VM ni cambiar el corpus de resultados. Registrar
Node.js y revisión del snapshot utilizados.

## Normas de clasificación

- Conservar los conteos `passed/failed/unsupported` que devuelve el runner.
- No convertir un `failed` en `unsupported` para inflar el porcentaje.
- No declarar un fallo `test262` como falso positivo sin evidencia
  independiente de la especificación y, preferentemente, de otro motor.
- Toda corrección de semántica debe tener regresión Tree/VM y GC stress, y
  ejecutarse con el `full` de la rama integrada.
