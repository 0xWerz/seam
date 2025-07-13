package shop;

import org.springframework.web.bind.annotation.*;

@RestController
public class Orders {
    private final OrderRepo repo;

    public Orders(OrderRepo repo) { this.repo = repo; }

    @GetMapping("/orders/{id}")
    @PreAuthorize("hasRole('ADMIN')")
    public Order show(@PathVariable String id) {
        return repo.find(id);
    }

    private void audit(String id) { System.out.println(id); }
}
