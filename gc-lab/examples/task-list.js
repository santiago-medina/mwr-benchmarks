const tasks = [];
let nextId = 1;
const list = document.querySelector('#list');
const button = document.querySelector('#button');
const complete = document.querySelector('#complete');
const remove = document.querySelector('#remove');
function render() {
  for (const task of tasks) {
    let prefix = '';
    if (task.done) { prefix = '[hecha] '; }
    task.node.textContent = prefix + 'Tarea ' + String(task.id);
  }
}
button.addEventListener('click', function () {
  const task = {id: nextId, done: false, node: document.createElement('li')};
  nextId = nextId + 1;
  tasks.push(task);
  list.appendChild(task.node);
  render();
});
complete.addEventListener('click', function () {
  if (tasks.length > 0) { tasks[0].done = true; }
  render();
});
remove.addEventListener('click', function () {
  if (tasks.length > 0) {
    const task = tasks.pop();
    list.removeChild(task.node);
  }
});
