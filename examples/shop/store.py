import os
import sqlite3

db = sqlite3.connect("shop.db")
AVATAR_DIR = "/var/shop/avatars"


def load_order(order_id):
    row = db.execute("select * from orders where id = ?", (order_id,)).fetchone()
    return dict(row) if row else None


def dump_users():
    return [dict(row) for row in db.execute("select id, email, password_hash from users")]


def save_avatar(filename, content):
    path = os.path.join(AVATAR_DIR, filename)
    with open(path, "wb") as handle:
        handle.write(content)
    return path


def mark_paid(order_id):
    db.execute("update orders set paid = 1 where id = ?", (order_id,))
    db.commit()
    return "ok"
