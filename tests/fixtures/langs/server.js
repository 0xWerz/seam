const express = require("express");
const { requireAuth } = require("./auth");
const app = express();

app.get("/health", (req, res) => {
  res.send("ok");
});

app.post("/users", requireAuth, async (req, res) => {
  const user = await createUser(req.body);
  res.json(user);
});

const listOrders = async (req, res) => {
  res.json(await db.query("select * from orders"));
};
app.get("/orders", requireAuth, listOrders);

function createUser(body) {
  return db.insert("users", body);
}

class Mailer {
  constructor(key) { this.key = key; }
  send(to, text) { return fetch("https://mail/" + to, { body: text }); }
  static create() { return new Mailer(process.env.KEY); }
}
module.exports = { app, Mailer };
