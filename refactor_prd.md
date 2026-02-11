# **PRD: Qorvex Native Core Refactor**

| Metadata | Details |
| :---- | :---- |
| **Project** | Qorvex (iOS Automation Tool) |
| **Target System** | iOS (Simulators & Real Devices) |
| **Goal** | Replace axe CLI dependency with native Rust/Swift implementation |
| **Status** | Draft |
| **Date** | October 26, 2023 |

## **1\. Executive Summary**

Currently, qorvex relies on axe (an external CLI tool) to drive iOS automation. This introduces process overhead, dependency management issues, and performance bottlenecks.

This initiative replaces axe with a **custom "Puppeteer" architecture**. We will build a lightweight Swift "Agent" that runs on the iOS target and listens for commands from the Rust qorvex host over a direct TCP socket. This unifies Simulator and Device interaction into a single high-performance API.

## **2\. Problem Statement**

* **Performance:** Shelling out to a CLI (axe) for every action adds significant latency (process startup \+ parsing standard output).  
* **Fragility:** Parsing text output from CLI tools is brittle and breaks with Xcode/OS updates.  
* **Simulator/Device Drift:** Handling devices and simulators often requires different code paths or tools in the current implementation.  
* **Dependency Hell:** Users must install axe and its dependencies separately, increasing friction.

## **3\. Proposed Architecture**

### **3.1. High-Level Diagram**

\[ Qorvex Rust Host \]  \<-- TCP (Binary/Protobuf) \--\>  \[ Swift Agent (iOS) \]  
       |                                                    |  
       \+-- (USB Tunnel / Localhost) \------------------------+

### **3.2. Components**

#### **A. The Host (Rust)**

* **Role:** The brain. Manages connections, serializes commands, and handles business logic.  
* **Key Libraries:**  
  * idevice (via rusty\_libimobiledevice or pure Rust implementation) for USB tunneling/port forwarding.  
  * tokio for async TCP communication.  
  * simctl (optional, for simulator lifecycle only).

#### **B. The Agent (Swift)**

* **Role:** The muscle. A minimal iOS app (UI Testing Target) that links against XCTest.  
* **Mechanism:**  
  * Runs a TCPServer on port 8080\.  
  * Translates binary OpCodes directly into XCUIElement actions.  
  * **No HTTP/JSON overhead.** Pure bytes.

## **4\. Functional Requirements**

### **4.1. Core Automation Actions (The "Unified API")**

The Rust Trait must standardize these actions for both Sim and Device.

| Feature | Description | Implementation Detail |
| :---- | :---- | :---- |
| **Tap (Coords)** | Tap at specific (x, y). | Send Tap(x, y) → Agent calls coordinate(withNormalizedOffset:...).tap() |
| **Tap (ID)** | Tap element by Accessibility ID. | Send TapID("submit\_btn") → Agent calls app.buttons\["submit\_btn"\].tap() |
| **Type Text** | Send keyboard input. | Send Type("hello") → Agent calls typeText("hello") |
| **Swipe** | Drag from A to B. | Send Swipe(x1, y1, x2, y2, duration) |
| **Tree Dump** | Get full UI hierarchy. | Send Dump → Agent returns compressed debugDescription or serialized tree. |
| **Screenshot** | Capture screen. | **Device:** idevice\_screenshot (USB raw stream). **Sim:** Agent returns JPEG bytes. |

### **4.2. Connection Management**

* **Device Discovery:** Qorvex must list connected devices and running simulators natively.  
* **Agent Installation:** Qorvex must automatically install the Agent (.ipa or .app) if it is missing or outdated.  
* **Agent Launch:** Qorvex must launch the Agent using xcrun simctl launch (Sim) or idevice-debug equivalent (Device) to start the server.

## **5\. Technical Specifications**

### **5.1. The Protocol (Binary)**

To ensure maximum speed, we will use a custom binary format (or lightweight Protobuf). Avoid JSON.

**Draft Packet Structure (Little Endian):**

\[Header: 4 bytes len\] \[OpCode: 1 byte\] \[Payload: Variable\]

**OpCodes:**

* 0x01: Heartbeat / Ping  
* 0x02: Tap Coordinate \[u32: x, u32: y\]  
* 0x03: Tap Element \[u32: id\_len, bytes: id\_string\]  
* 0x10: Get Tree (Returns XML or Protobuf)  
* 0x99: Error \[u32: msg\_len, bytes: msg\]

### **5.2. Rust Trait Definition**

\#\[async\_trait\]  
pub trait AutomationDriver {  
    /// Establishes the TCP connection (direct or via USB tunnel)  
    async fn connect(\&mut self) \-\> Result\<()\>;  
      
    /// Performs a tap at the specific X/Y coordinates  
    async fn tap(\&self, x: u32, y: u32) \-\> Result\<()\>;  
      
    /// Types text into the currently focused element  
    async fn type\_text(\&self, text: \&str) \-\> Result\<()\>;  
      
    /// Returns the raw bytes of a screenshot (PNG/JPEG)  
    async fn screenshot(\&self) \-\> Result\<Vec\<u8\>\>;  
      
    /// Returns the UI hierarchy (Accessibility Tree)  
    async fn dump\_tree(\&self) \-\> Result\<String\>;  
}

## **6\. Implementation Phases**

### **Phase 1: The Prototype (Simulator Only)**

* **Goal:** Prove the TCP concept works without USB complexity.  
* **Tasks:**  
  1. Create qorvex-agent (Swift) that listens on localhost:8080.  
  2. Implement Tap and Dump OpCodes in Swift.  
  3. Write Rust client (TcpStream::connect("127.0.0.1:8080")) to send commands.

### **Phase 2: The USB Tunnel (Device Support)**

* **Goal:** Get it working on a real iPhone.  
* **Tasks:**  
  1. Integrate idevice crate into Qorvex.  
  2. Implement usbmuxd port forwarding (Map local random port → Device 8080).  
  3. Abstract the connection logic so AutomationDriver uses the tunnel for devices.

### **Phase 3: Parity & Replacement**

* **Goal:** Feature completeness.  
* **Tasks:**  
  1. Implement Swipe, Scroll, and complex gestures.  
  2. Implement automatic Agent installation (handling code signing for devices).  
  3. **Cutover:** Remove axe calls from the main Qorvex codebase.

## **7\. Success Metrics**

* **Latency:** Action round-trip time (RTT) \< 50ms (Target: 10x improvement over CLI).  
* **Stability:** Pass rate of 99.9% on connection attempts.  
* **Dependency:** Zero external CLI tools required (users just download the Qorvex binary).

## **8\. Risks & Mitigations**

* **Risk:** Code Signing. Real devices require the Agent to be signed.  
  * *Mitigation:* Qorvex can resign the Agent .ipa on the fly using user's certificates (similar to how Appium/Flutter does it), or require the user to build the Agent once in Xcode with their Team ID.  
* **Risk:** XCTest stability. The Agent might crash or be killed by iOS memory management.  
  * *Mitigation:* Implement a robust "Keep-Alive" or "Watchdog" in Rust to relaunch the Agent immediately if the socket connection is lost.
