import Foundation

func normalizeOrderId(_ orderId: String) -> String {
    return orderId.trimmingCharacters(in: .whitespacesAndNewlines)
}

final class OrderService {
    let prefix: String

    init(prefix: String = "order") {
        self.prefix = prefix
    }

    func displayName(_ orderId: String) -> String {
        let normalized = normalizeOrderId(orderId)
        return "\(prefix):\(normalized)"
    }

    func summary(_ orderId: String) -> String {
        return formatOrder(displayName(orderId))
    }

    private func formatOrder(_ value: String) -> String {
        return value.uppercased()
    }
}
