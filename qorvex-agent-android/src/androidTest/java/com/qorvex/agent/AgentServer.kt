// AgentServer.kt
// Blocking TCP server that accepts connections from the Rust host (over the
// adb-forwarded localhost port) and dispatches binary-protocol frames to the
// CommandHandler. Mirrors the framing and one-connection-at-a-time behavior of
// the Swift AgentServer (NWListener).

package com.qorvex.agent

import android.util.Log
import java.io.DataInputStream
import java.io.InputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket

class AgentServer(
    private val port: Int,
    private val handler: CommandHandler,
) {
    private var serverSocket: ServerSocket? = null

    companion object {
        private const val TAG = "qorvex-agent"
        // Guard against pathological/garbage length headers (64 MiB cap).
        private const val MAX_FRAME_LENGTH = 64 * 1024 * 1024
    }

    /**
     * Bind and serve forever. Accepts one connection at a time; when a client
     * disconnects, loops back to accept the next. Blocks the calling thread.
     */
    fun serveForever() {
        val socket = ServerSocket()
        socket.reuseAddress = true
        // Bind to localhost only — reached via `adb forward`.
        socket.bind(java.net.InetSocketAddress(InetAddress.getByName("127.0.0.1"), port))
        serverSocket = socket
        Log.i(TAG, "Server listening on 127.0.0.1:$port")

        while (!socket.isClosed) {
            val client = try {
                socket.accept()
            } catch (e: Exception) {
                if (socket.isClosed) break
                Log.e(TAG, "accept failed: ${e.message}")
                continue
            }
            try {
                client.tcpNoDelay = true
                handleConnection(client)
            } catch (e: Exception) {
                Log.e(TAG, "connection error: ${e.message}")
            } finally {
                try { client.close() } catch (_: Exception) {}
            }
        }
    }

    fun stop() {
        try { serverSocket?.close() } catch (_: Exception) {}
        serverSocket = null
    }

    private fun handleConnection(client: Socket) {
        Log.i(TAG, "Client connected")
        val input = DataInputStream(client.getInputStream())
        val output = client.getOutputStream()

        while (true) {
            val payload = readFrame(input) ?: break // peer closed
            if (payload.isEmpty()) continue // zero-length frame; skip

            val response: AgentResponse = try {
                handler.handle(decodeRequest(payload))
            } catch (e: ProtocolException) {
                AgentResponse.Error("decode error: ${e.message}")
            } catch (e: Exception) {
                AgentResponse.Error("agent error: ${e.message ?: e.javaClass.simpleName}")
            }

            try {
                output.write(encodeResponse(response))
                output.flush()
            } catch (e: Exception) {
                Log.e(TAG, "send failed: ${e.message}")
                break
            }
        }
        Log.i(TAG, "Client disconnected")
    }

    /**
     * Read one complete frame: 4-byte LE length header, then `length` payload
     * bytes. Returns the payload (opcode + body), or null when the peer closes.
     */
    private fun readFrame(input: DataInputStream): ByteArray? {
        val header = ByteArray(4)
        if (!readFully(input, header, 4)) return null
        val len = readFrameLength(header)
        if (len < 0 || len > MAX_FRAME_LENGTH) {
            throw ProtocolException.InvalidPayload("frame length out of range: $len")
        }
        if (len == 0L) return ByteArray(0)
        val payload = ByteArray(len.toInt())
        if (!readFully(input, payload, len.toInt())) return null
        return payload
    }

    /** Fill `buf[0..n]`. Returns false if the stream ends before `n` bytes. */
    private fun readFully(input: InputStream, buf: ByteArray, n: Int): Boolean {
        var off = 0
        while (off < n) {
            val r = input.read(buf, off, n - off)
            if (r < 0) return false
            off += r
        }
        return true
    }
}
