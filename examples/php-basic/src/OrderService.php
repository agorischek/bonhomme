<?php

namespace Order;

function normalize_order_id(string $orderId): string
{
    return trim($orderId);
}

final class OrderService
{
    private string $prefix = 'order';

    public function displayName(string $orderId): string
    {
        return $this->prefix . ':' . normalize_order_id($orderId);
    }

    public function summary(string $orderId): string
    {
        return self::formatOrder($this->displayName($orderId));
    }

    private static function formatOrder(string $value): string
    {
        return strtoupper($value);
    }
}
