package order

type OrderService struct {
	ServiceName string
}

func FormatOrder(id string) string {
	return "order:" + id
}

func (s *OrderService) DisplayName() string {
	return s.ServiceName
}

func (s *OrderService) Summary(id string) string {
	return s.DisplayName() + ":" + FormatOrder(id)
}
