<?php
class OrdersController {
    #[Route('/orders/{id}')]
    public function show(string $id) {
        return $this->repo->find($id);
    }
}
function helper_fn($x) { return strtoupper($x); }
