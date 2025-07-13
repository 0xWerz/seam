import { Controller, Get, UseGuards, Param } from "@nestjs/common";
import { AdminGuard } from "./guards";

interface Order { id: string; }

@Controller("orders")
export class OrdersController {
  constructor(private readonly orders: OrderService) {}

  @Get(":id")
  @UseGuards(AdminGuard)
  async findOne(@Param("id") id: string): Promise<Order> {
    return this.orders.load(id);
  }

  private audit(id: string): void {
    console.log(id);
  }
}

export const handler = async (event: unknown) => {
  return parse(event);
};

export function parse(event: unknown): string {
  return JSON.stringify(event);
}
