#pragma once

#include <cstdint>

#if __cplusplus >= 202302L
    #include <expected>
    #include <span>
    #include <variant>
#endif

namespace v5gdb {
namespace impl {
    /// A custom transport method for communicating with GDB.
    struct TransportImpl {
        /// Custom data, passed to each function.
        void* data;
        /// One-time initialize callback. Called on first breakpoint.
        void (*initialize)(void* data);
        /// Write a buffer containing packet data to GDB.
        ///
        /// Returns a static error string if an error occurred, or null if the
        /// operation was successful.
        char const* (*write_buf)(void* data, const uint8_t* buf, uintptr_t len);
        /// Flushes any pending writes to GDB.
        ///
        /// Returns a static error string if an error occurred, or null if the
        /// operation was successful.
        char const* (*flush)(void* data);
        /// Peeks the next byte received from GDB.
        ///
        /// Returns -1 if there are no bytes to read, or returns the unsigned byte.
        /// Sets *error to a static error string if an error occurred.
        int32_t (*peek_byte)(void* data, char const** error);
        /// Reads the next byte received from GDB.
        ///
        /// Sets *error to a static error string if an error occurred.
        uint8_t (*read_byte)(void* data, char const** error);
    };

    extern "C" {
    /// Install the debugger, communicating with GDB over the V5's USB serial port.
    void v5gdb_install_stdio();

    /// Install the debugger with a custom transport method for communicating with
    /// GDB.
    void v5gdb_install_custom(TransportImpl transport);

    /// Manually triggers a breakpoint.
    void v5gdb_breakpoint();
    } // extern "C"
} // namespace impl

class BaseTransport {
  public:
    virtual ~BaseTransport() noexcept = default;

  private:
    virtual void install() const = 0;
    friend void install(BaseTransport const& transport);
};

#if __cplusplus >= 202302L
// A subclassable transport for custom implementations.
class Transport: public BaseTransport {
  public:
    virtual void initialize() noexcept = 0;

    [[nodiscard]]
    virtual std::expected<std::monostate, char const*>
    write(std::span<uint8_t const> buffer) noexcept = 0;

    [[nodiscard]]
    virtual std::expected<std::monostate, char const*> flush() noexcept = 0;

    [[nodiscard]]
    virtual std::expected<std::uint8_t, char const*> peek() noexcept = 0;

    [[nodiscard]]
    virtual std::expected<std::uint8_t, char const*> read() noexcept = 0;

    ~Transport() noexcept override = default;

    [[nodiscard]]
    impl::TransportImpl as_impl() const noexcept {
        return impl::TransportImpl {
            .data = static_cast<void*>(const_cast<Transport*>(this)),
            .initialize = initialize_trampoline,
            .write_buf = write_trampoline,
            .flush = flush_trampoline,
            .peek_byte = peek_trampoline,
            .read_byte = read_trampoline,
        };
    }

  private:
    static void initialize_trampoline(void* data) {
        static_cast<Transport*>(data)->initialize();
    }

    static char const*
    write_trampoline(void* data, const uint8_t* buf, uintptr_t const len) {
        if (auto result =
                static_cast<Transport*>(data)->write(std::span(buf, len));
            result.has_value()) {
            return nullptr;
        } else {
            return result.error();
        }
    }

    static char const* flush_trampoline(void* data) {
        if (auto result = static_cast<Transport*>(data)->flush();
            result.has_value()) {
            return nullptr;
        } else {
            return result.error();
        }
    }

    static int32_t peek_trampoline(void* data, char const** error) {
        if (auto result = static_cast<Transport*>(data)->peek();
            result.has_value()) {
            return result.value();
        } else {
            *error = result.error();
            return -1;
        }
    }

    static uint8_t read_trampoline(void* data, char const** error) {
        if (auto result = static_cast<Transport*>(data)->read();
            result.has_value()) {
            return result.value();
        } else {
            *error = result.error();
            return 0;
        }
    }

    void install() const override {
        impl::v5gdb_install_custom(this->as_impl());
    }
};
#endif

// A builtin transport that uses the V5's USB serial port to communicate with GDB.
class StdioTransport: public BaseTransport {
  public:
    StdioTransport() = default;

  private:
    void install() const override {
        impl::v5gdb_install_stdio();
    }
};

// Install the debugger, using the specified transport to communicate with GDB.
inline void install(BaseTransport const& transport) {
    transport.install();
}

/// Manually triggers a breakpoint.
inline void breakpoint() {
    __asm__ volatile("bkpt");
}
} // namespace v5gdb
