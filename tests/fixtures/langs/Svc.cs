using Microsoft.AspNetCore.Mvc;
namespace Shop {
    public class OrdersController : ControllerBase {
        [HttpGet("orders/{id}")]
        [Authorize(Roles = "Admin")]
        public IActionResult Show(string id) {
            return Ok(Load(id));
        }
        private string Load(string id) { return id; }
    }
}
