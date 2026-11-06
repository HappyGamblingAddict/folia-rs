package rs.folia.bridge;

import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;

final class BridgeValues {
    private static final char[] HEX = "0123456789abcdef".toCharArray();

    private BridgeValues() {
    }

    static Object defaultValue(Method method) {
        return defaultValue(method.getReturnType());
    }

    static Object defaultValue(Class<?> type) {
        if (!type.isPrimitive()) {
            return null;
        }
        if (type == void.class) {
            return null;
        }
        if (type == boolean.class) {
            return false;
        }
        if (type == char.class) {
            return '\0';
        }
        if (type == byte.class) {
            return (byte) 0;
        }
        if (type == short.class) {
            return (short) 0;
        }
        if (type == int.class) {
            return 0;
        }
        if (type == long.class) {
            return 0L;
        }
        if (type == float.class) {
            return 0.0F;
        }
        if (type == double.class) {
            return 0.0D;
        }
        return null;
    }

    /** Hex-encode UTF-8 text so tab-separated bridge fields stay unambiguous. */
    static String hex(String value) {
        if (value == null || value.isEmpty()) {
            return "";
        }
        byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
        StringBuilder encoded = new StringBuilder(bytes.length * 2);
        for (byte current : bytes) {
            encoded.append(HEX[(current >> 4) & 0x0f]).append(HEX[current & 0x0f]);
        }
        return encoded.toString();
    }

    static String unhex(String value) {
        if (value == null || value.isEmpty()) {
            return "";
        }
        if ((value.length() & 1) != 0) {
            return "";
        }
        byte[] bytes = new byte[value.length() / 2];
        for (int index = 0; index < bytes.length; index++) {
            int high = Character.digit(value.charAt(index * 2), 16);
            int low = Character.digit(value.charAt(index * 2 + 1), 16);
            if (high < 0 || low < 0) {
                return "";
            }
            bytes[index] = (byte) ((high << 4) | low);
        }
        return new String(bytes, StandardCharsets.UTF_8);
    }
}
