function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
if (fib(22) !== 17711) throw new Error('fib');
