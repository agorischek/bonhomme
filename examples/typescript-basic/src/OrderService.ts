type OrderId = string;

export function formatOrder(id: OrderId): string {
  return id.toUpperCase();
}

export class OrderService {
  private prefix: string = "order";

  displayName(id: OrderId): string {
    return `${this.prefix}:${formatOrder(id)}`;
  }

  summary(id: OrderId): string {
    return this.displayName(id);
  }
}
