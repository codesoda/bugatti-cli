from flask import Flask, jsonify

app = Flask(__name__)

ITEMS = [
    {"id": 1, "name": "Widget", "price": 9.99},
    {"id": 2, "name": "Gadget", "price": 24.99},
    {"id": 3, "name": "Doohickey", "price": 4.99},
]


@app.route("/health")
def health():
    return jsonify({"status": "ok"})


@app.route("/api/items")
def get_items():
    return jsonify(ITEMS)


@app.route("/api/items/<int:item_id>")
def get_item(item_id):
    item = next((i for i in ITEMS if i["id"] == item_id), None)
    if item is None:
        return jsonify({"error": "not found"}), 404
    return jsonify(item)


@app.route("/")
def index():
    return """<!DOCTYPE html>
<html>
<head><title>Item Store</title></head>
<body>
  <h1>Item Store</h1>
  <ul id="items"></ul>
  <script>
    fetch('/api/items')
      .then(r => r.json())
      .then(items => {
        const ul = document.getElementById('items');
        items.forEach(item => {
          const li = document.createElement('li');
          li.textContent = `${item.name} - $${item.price.toFixed(2)}`;
          ul.appendChild(li);
        });
      });
  </script>
</body>
</html>"""


if __name__ == "__main__":
    app.run(port=5111)
