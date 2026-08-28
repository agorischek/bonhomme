package order;

import java.util.Locale;

public final class OrderService {
    private final String prefix = "order";

    public String displayName(String orderId) {
        return prefix + ":" + orderId;
    }

    public String summary(String orderId) {
        return formatOrder(displayName(orderId));
    }

    private static String formatOrder(String value) {
        return value.toUpperCase(Locale.ROOT);
    }

    enum Status {
        OPEN,
        CLOSED;
    }
}
