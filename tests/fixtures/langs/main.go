package main

import "net/http"

type Server struct{ db *DB }

// handleExport writes every user.
func (s *Server) handleExport(w http.ResponseWriter, r *http.Request) {
	rows := s.db.AllUsers()
	writeJSON(w, rows)
}

func requireAdmin(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("X-Admin") == "" {
			http.Error(w, "no", 403)
			return
		}
		next(w, r)
	}
}

func main() {
	s := &Server{}
	http.HandleFunc("/export", requireAdmin(s.handleExport))
}

func writeJSON(w http.ResponseWriter, v any) {}
