// AgentServer.swift
// TCP server using NWListener (Network framework) that accepts connections
// from the Rust host and dispatches binary protocol commands.

import Foundation
import Network

final class AgentServer {
    private let port: UInt16
    private var listener: NWListener?
    private var activeConnection: NWConnection?
    private let handler: CommandHandler
    private let queue = DispatchQueue(label: "com.qorvex.agent.server")

    init(port: UInt16, handler: CommandHandler) {
        self.port = port
        self.handler = handler
    }

    /// Start listening on the configured TCP port.
    func start() throws {
        let params = NWParameters.tcp
        params.allowLocalEndpointReuse = true

        guard let nwPort = NWEndpoint.Port(rawValue: port) else {
            throw AgentServerError.invalidPort(port)
        }

        let listener = try NWListener(using: params, on: nwPort)
        self.listener = listener

        listener.stateUpdateHandler = { [weak self] state in
            switch state {
            case .ready:
                if let port = self?.listener?.port {
                    NSLog("[qorvex-agent] Server listening on port %d", port.rawValue)
                }
            case .failed(let error):
                NSLog("[qorvex-agent] Listener failed: %@", error.localizedDescription)
                self?.stop()
            case .cancelled:
                NSLog("[qorvex-agent] Listener cancelled")
            default:
                break
            }
        }

        listener.newConnectionHandler = { [weak self] connection in
            self?.handleNewConnection(connection)
        }

        listener.start(queue: queue)
    }

    /// Stop the server and close any active connection.
    func stop() {
        activeConnection?.cancel()
        activeConnection = nil
        listener?.cancel()
        listener = nil
        NSLog("[qorvex-agent] Server stopped")
    }

    // MARK: - Connection handling

    private func handleNewConnection(_ connection: NWConnection) {
        // Only allow one connection at a time.
        if let existing = activeConnection {
            NSLog("[qorvex-agent] Replacing existing connection")
            existing.cancel()
        }
        activeConnection = connection

        connection.stateUpdateHandler = { [weak self] state in
            switch state {
            case .ready:
                NSLog("[qorvex-agent] Client connected")
                self?.receiveFrame(on: connection)
            case .failed(let error):
                NSLog("[qorvex-agent] Connection failed: %@", error.localizedDescription)
                self?.connectionEnded(connection)
            case .cancelled:
                NSLog("[qorvex-agent] Connection cancelled")
                self?.connectionEnded(connection)
            default:
                break
            }
        }

        connection.start(queue: queue)
    }

    private func connectionEnded(_ connection: NWConnection) {
        if activeConnection === connection {
            activeConnection = nil
        }
    }

    // MARK: - Frame reading

    /// Read a complete frame: 4-byte LE length header, then `length` bytes of payload.
    private func receiveFrame(on connection: NWConnection) {
        // Step 1: read the 4-byte length header.
        connection.receive(minimumIncompleteLength: 4, maximumLength: 4) {
            [weak self] headerData, _, isComplete, error in

            guard let self = self else { return }

            if let error = error {
                NSLog("[qorvex-agent] Read header error: %@", error.localizedDescription)
                connection.cancel()
                return
            }

            if isComplete && (headerData == nil || headerData!.isEmpty) {
                NSLog("[qorvex-agent] Connection closed by peer")
                connection.cancel()
                return
            }

            guard let headerData = headerData, headerData.count == 4 else {
                NSLog("[qorvex-agent] Incomplete header (%d bytes)", headerData?.count ?? 0)
                connection.cancel()
                return
            }

            let payloadLength = Int(readFrameLength(headerData))
            if payloadLength == 0 {
                NSLog("[qorvex-agent] Received zero-length frame")
                self.receiveFrame(on: connection)
                return
            }

            // Step 2: read the payload.
            self.receivePayload(length: payloadLength, on: connection)
        }
    }

    private func receivePayload(length: Int, on connection: NWConnection) {
        connection.receive(minimumIncompleteLength: length, maximumLength: length) {
            [weak self] payloadData, _, isComplete, error in

            guard let self = self else { return }

            if let error = error {
                NSLog("[qorvex-agent] Read payload error: %@", error.localizedDescription)
                connection.cancel()
                return
            }

            guard let payloadData = payloadData, payloadData.count == length else {
                NSLog("[qorvex-agent] Incomplete payload (expected %d, got %d)",
                      length, payloadData?.count ?? 0)
                connection.cancel()
                return
            }

            // Decode and handle the request.
            let response: AgentResponse
            do {
                let request = try decodeRequest(from: payloadData)
                response = self.handler.handle(request)
            } catch {
                NSLog("[qorvex-agent] Decode error: %@", "\(error)")
                response = .error(message: "decode error: \(error)")
            }

            // Send the response, then read next frame.
            self.sendResponse(response, on: connection) {
                self.receiveFrame(on: connection)
            }
        }
    }

    // MARK: - Sending responses

    private func sendResponse(
        _ response: AgentResponse,
        on connection: NWConnection,
        completion: @escaping () -> Void
    ) {
        let wire = encodeResponse(response)
        connection.send(content: wire, completion: .contentProcessed { error in
            if let error = error {
                NSLog("[qorvex-agent] Send error: %@", error.localizedDescription)
                connection.cancel()
                return
            }
            completion()
        })
    }
}

// MARK: - Server errors

enum AgentServerError: Error, CustomStringConvertible {
    case invalidPort(UInt16)

    var description: String {
        switch self {
        case .invalidPort(let port):
            return "Invalid port number: \(port)"
        }
    }
}
