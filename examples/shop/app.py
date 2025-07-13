"""A small web app with a few deliberate gaps, used to demonstrate seam."""
from flask import Flask, request, jsonify

from auth import require_admin, require_login
from fetch import download
from store import dump_users, load_order, mark_paid, save_avatar

app = Flask(__name__)


@app.route("/preview")
def preview_link():
    url = request.args["url"]
    return download(url)


@app.route("/orders/<order_id>")
@require_login
def show_order(order_id):
    return jsonify(load_order(order_id))


@app.route("/admin/export")
@require_admin
def export_users():
    return jsonify(dump_users())


@app.route("/avatar", methods=["POST"])
@require_login
def upload_avatar():
    upload = request.files["avatar"]
    return save_avatar(upload.filename, upload.read())


@app.route("/hooks/payment", methods=["POST"])
def payment_webhook():
    event = request.get_json()
    return mark_paid(event["order_id"])
