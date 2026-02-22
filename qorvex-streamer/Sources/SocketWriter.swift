// SocketWriter.swift
// Manages a Unix domain socket server that accepts a single client
// and writes length-prefixed JPEG frames using 4-byte LE u32 framing.

import Foundation

enum SocketWriterError: Error, CustomStringConvertible {
    case socketCreationFailed
    case bindFailed(path: String, errno: Int32)
    case listenFailed(errno: Int32)
    case acceptFailed(errno: Int32)

    var description: String {
        switch self {
        case .socketCreationFailed:
            return "Failed to create Unix domain socket"
        case .bindFailed(let path, let code):
            return "Failed to bind socket at \(path): \(String(cString: strerror(code)))"
        case .listenFailed(let code):
            return "Failed to listen on socket: \(String(cString: strerror(code)))"
        case .acceptFailed(let code):
            return "Failed to accept connection: \(String(cString: strerror(code)))"
        }
    }
}

final class SocketWriter {
    private let socketPath: String
    private var serverFd: Int32 = -1
    private var clientFd: Int32 = -1
    private let lock = NSLock()

    init(socketPath: String) {
        self.socketPath = socketPath
    }

    /// Create, bind, and listen on the Unix domain socket.
    func bind() throws {
        // Remove stale socket file if it exists.
        unlink(socketPath)

        serverFd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard serverFd >= 0 else {
            throw SocketWriterError.socketCreationFailed
        }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)

        // Copy the socket path into sun_path.
        let pathBytes = socketPath.utf8CString
        let maxLen = MemoryLayout.size(ofValue: addr.sun_path)
        precondition(pathBytes.count <= maxLen, "Socket path too long (max \(maxLen - 1) chars)")

        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            ptr.withMemoryRebound(to: CChar.self, capacity: maxLen) { dest in
                for i in 0..<pathBytes.count {
                    dest[i] = pathBytes[i]
                }
            }
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)
        let bindResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                Darwin.bind(serverFd, sockPtr, addrLen)
            }
        }
        guard bindResult == 0 else {
            throw SocketWriterError.bindFailed(path: socketPath, errno: errno)
        }

        guard listen(serverFd, 1) == 0 else {
            throw SocketWriterError.listenFailed(errno: errno)
        }
    }

    /// Block until a client connects. Stores the client file descriptor.
    func acceptClient() {
        var clientAddr = sockaddr_un()
        var clientAddrLen = socklen_t(MemoryLayout<sockaddr_un>.size)

        let fd = withUnsafeMutablePointer(to: &clientAddr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                accept(serverFd, sockPtr, &clientAddrLen)
            }
        }

        if fd >= 0 {
            lock.lock()
            clientFd = fd
            lock.unlock()
        } else {
            NSLog("[qorvex-streamer] accept() failed: %s", strerror(errno))
        }
    }

    /// Write a frame with 4-byte little-endian length prefix followed by JPEG data.
    /// Calls completion when done (or on error). On broken pipe, closes the client
    /// and waits for a new connection.
    func writeFrame(_ data: Data, completion: @escaping () -> Void) {
        lock.lock()
        let fd = clientFd
        lock.unlock()

        guard fd >= 0 else {
            completion()
            return
        }

        // Build the length header (4-byte LE u32).
        var length = UInt32(data.count).littleEndian
        let headerData = Data(bytes: &length, count: 4)

        let success = writeAll(fd: fd, data: headerData) && writeAll(fd: fd, data: data)

        if !success {
            NSLog("[qorvex-streamer] Write failed (broken pipe), waiting for reconnection")
            lock.lock()
            Darwin.close(clientFd)
            clientFd = -1
            lock.unlock()

            // Wait for a new client on a background thread.
            DispatchQueue.global().async { [weak self] in
                self?.acceptClient()
                NSLog("[qorvex-streamer] Client reconnected")
            }
        }

        completion()
    }

    /// Clean up the socket file and close file descriptors.
    func close() {
        lock.lock()
        if clientFd >= 0 {
            Darwin.close(clientFd)
            clientFd = -1
        }
        if serverFd >= 0 {
            Darwin.close(serverFd)
            serverFd = -1
        }
        lock.unlock()
        unlink(socketPath)
    }

    // MARK: - Private

    /// Write all bytes of `data` to `fd`, handling partial writes.
    private func writeAll(fd: Int32, data: Data) -> Bool {
        return data.withUnsafeBytes { buffer -> Bool in
            guard let ptr = buffer.baseAddress else { return false }
            var offset = 0
            let total = data.count
            while offset < total {
                let written = Darwin.write(fd, ptr.advanced(by: offset), total - offset)
                if written <= 0 {
                    return false
                }
                offset += written
            }
            return true
        }
    }
}
