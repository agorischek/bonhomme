using System;

namespace Bonhomme.Example
{
    public sealed class OrderService
    {
        private readonly string _prefix = "order";

        public string DisplayName(string orderId)
        {
            return $"{_prefix}:{orderId}";
        }

        public string Summary(string orderId)
        {
            return FormatOrder(DisplayName(orderId));
        }

        private static string FormatOrder(string value)
        {
            return value.ToUpperInvariant();
        }
    }
}
