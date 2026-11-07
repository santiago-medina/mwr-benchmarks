# MWR Benchmarks

Benchmark comparativo de C++ y Rust ejecutado en GitHub Actions.

El workflow compila las implementaciones optimizadas, comprueba que los resultados coinciden y compara tiempos de ejecución mediante medianas de nueve repeticiones.

## Laboratorio de memoria

[gc-lab](gc-lab/README.md) compara tres contratos de memoria para un futuro motor JavaScript: slots estables, compactación directa y compactación mediante IDs estables. Incluye raíces por ámbito, un anfitrión DOM/JS simulado, recuperación ante cuotas de objetos y pruebas automáticas en debug y release.
