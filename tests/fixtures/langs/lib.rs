use actix_web::{get, web, HttpResponse};

pub struct Store<T> { inner: T }

impl<T> Store<T> {
    pub fn new(inner: T) -> Self { Self { inner } }

    /// Reads one row.
    pub fn load(&self, id: u64) -> Option<String> { None }
}

#[get("/orders/{id}")]
#[require_role("admin")]
pub async fn show_order(path: web::Path<u64>) -> HttpResponse {
    let row = fetch_row(path.into_inner());
    HttpResponse::Ok().body(row)
}

fn fetch_row(id: u64) -> String { String::new() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loads() { assert!(fetch_row(1).is_empty()); }
}
