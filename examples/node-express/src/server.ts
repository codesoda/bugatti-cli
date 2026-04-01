import express from "express";

const app = express();
const PORT = 3111;

interface Todo {
  id: number;
  title: string;
  completed: boolean;
}

const todos: Todo[] = [
  { id: 1, title: "Buy groceries", completed: false },
  { id: 2, title: "Write tests", completed: true },
  { id: 3, title: "Deploy app", completed: false },
];

app.use(express.json());

app.get("/health", (_req, res) => {
  res.json({ status: "ok", uptime: process.uptime() });
});

app.get("/api/todos", (_req, res) => {
  res.json(todos);
});

app.post("/api/todos", (req, res) => {
  const { title } = req.body;
  if (!title || typeof title !== "string") {
    res.status(400).json({ error: "title is required" });
    return;
  }
  const todo: Todo = {
    id: todos.length + 1,
    title,
    completed: false,
  };
  todos.push(todo);
  res.status(201).json(todo);
});

app.get("/", (_req, res) => {
  res.send(`<!DOCTYPE html>
<html>
<head><title>Todo App</title></head>
<body>
  <h1>Todo List</h1>
  <form id="add-form">
    <input type="text" id="new-todo" placeholder="New todo..." required>
    <button type="submit">Add</button>
  </form>
  <ul id="todo-list"></ul>
  <script>
    async function loadTodos() {
      const res = await fetch('/api/todos');
      const todos = await res.json();
      const ul = document.getElementById('todo-list');
      ul.innerHTML = '';
      todos.forEach(todo => {
        const li = document.createElement('li');
        li.textContent = todo.completed ? '[x] ' + todo.title : '[ ] ' + todo.title;
        ul.appendChild(li);
      });
    }

    document.getElementById('add-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const input = document.getElementById('new-todo');
      await fetch('/api/todos', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ title: input.value })
      });
      input.value = '';
      await loadTodos();
    });

    loadTodos();
  </script>
</body>
</html>`);
});

app.listen(PORT, () => {
  console.log(`Server running on http://localhost:${PORT}`);
});

// Admin server on a separate port
const admin = express();
const ADMIN_PORT = 3112;

admin.get("/health", (_req, res) => {
  res.json({ status: "ok", role: "admin" });
});

admin.get("/stats", (_req, res) => {
  res.json({
    todo_count: todos.length,
    completed_count: todos.filter((t) => t.completed).length,
    uptime: process.uptime(),
  });
});

admin.listen(ADMIN_PORT, () => {
  console.log(`Admin running on http://localhost:${ADMIN_PORT}`);
});
