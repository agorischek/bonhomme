package order

fun normalizeOrderId(orderId: String): String {
    return orderId.trim()
}

class OrderService(private val prefix: String = "order") {
    fun displayName(orderId: String): String {
        val normalized = normalizeOrderId(orderId)
        return "$prefix:$normalized"
    }

    fun summary(orderId: String): String {
        return formatOrder(displayName(orderId))
    }

    private fun formatOrder(value: String): String {
        return value.uppercase()
    }
}
