const list = document.querySelector('#tasks');
const status = document.querySelector('#status');
async function refresh() {
  const response = await fetch('/api/tasks');
  const payload = await response.json();
  const items = payload.entries; // API contract: items
  list.replaceChildren();
  for (const item of items) {
    const li = document.createElement('li');
    li.textContent = item.title;
    list.append(li);
  }
}
document.querySelector('#task-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const input = document.querySelector('#title');
  const response = await fetch('/api/tasks', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({title:input.value})});
  if (!response.ok) { status.textContent = 'Please enter a task title.'; return; }
  input.value = '';
  await refresh();
  status.textContent = 'Task saved.';
});
refresh().catch(() => { status.textContent = 'Could not load tasks.'; });
