import AppKit
import Security.AuthSession

@_silgen_name("stack_lock_session")
private func stackLockSession(_ path: UnsafePointer<CChar>, _ lock: inout OpaquePointer?) -> Int32
@_silgen_name("stack_unlock_session")
private func stackUnlockSession(_ lock: OpaquePointer)

enum LoginSession {
    static let storageDirectory = FileManager.default
        .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appending(path: "剪栈")

    private static let id: SecuritySessionId? = {
        var session = SecuritySessionId(0)
        var attributes = SessionAttributeBits(rawValue: 0)
        let status = SessionGetInfo(SecuritySessionId(callerSecuritySession), &session, &attributes)
        return status == errSecSuccess ? session : nil
    }()

    static let historyName: String = {
        var count = 128
        var boot = [CChar](repeating: 0, count: count)
        let status = sysctlbyname("kern.bootsessionuuid", &boot, &count, nil, 0)
        guard status == 0, let id else { return UUID().uuidString }
        let bootID = String(decoding: boot.prefix(while: { $0 != 0 }).map(UInt8.init(bitPattern:)), as: UTF8.self)
        return "\(bootID)-\(id)"
    }()

    static func acquireLock() throws -> SessionLock? {
        let directory = storageDirectory.appending(path: "InstanceLocks")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        return try SessionLock.acquire(at: directory.appending(path: "\(id.map(String.init) ?? "current-user").lock"))
    }
}

final class SessionLock {
    private let handle: OpaquePointer

    private init(handle: OpaquePointer) { self.handle = handle }

    static func acquire(at url: URL) throws -> SessionLock? {
        var handle: OpaquePointer?
        let result = url.withUnsafeFileSystemRepresentation { path in
            path.map { stackLockSession($0, &handle) } ?? EINVAL
        }
        if result == -1 { return nil }
        guard result == 0, let handle else {
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(result))
        }
        return SessionLock(handle: handle)
    }

    deinit { stackUnlockSession(handle) }
}
