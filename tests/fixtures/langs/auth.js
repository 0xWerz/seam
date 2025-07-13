function requireAuth(req, res, next) {
  if (!req.session.user) return res.status(401).end();
  next();
}
exports.requireAuth = requireAuth;
