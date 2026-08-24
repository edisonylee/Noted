import Foundation

#if canImport(UIKit)
  import UIKit
#endif

public enum ProtectedDataState: String, Codable, Equatable, Sendable {
  case available
  case unavailable
}

public struct ProtectedDataEvent: Codable, Equatable, Sendable {
  public let state: ProtectedDataState
  public let observedAtMs: Int64
}

public final class ProtectedDataMonitor: @unchecked Sendable {
  private let lock = NSLock()
  private var callback: (@Sendable (ProtectedDataEvent) -> Void)?
  private var observers: [NSObjectProtocol] = []

  public init() {
    #if os(iOS)
      let center = NotificationCenter.default
      observers.append(
        center.addObserver(
          forName: UIApplication.protectedDataWillBecomeUnavailableNotification,
          object: nil,
          queue: .main
        ) { [weak self] _ in
          self?.emit(.unavailable)
        })
      observers.append(
        center.addObserver(
          forName: UIApplication.protectedDataDidBecomeAvailableNotification,
          object: nil,
          queue: .main
        ) { [weak self] _ in
          self?.emit(.available)
        })
    #endif
  }

  deinit {
    for observer in observers {
      NotificationCenter.default.removeObserver(observer)
    }
  }

  public var currentState: ProtectedDataState {
    #if os(iOS)
      UIApplication.shared.isProtectedDataAvailable ? .available : .unavailable
    #else
      .available
    #endif
  }

  public func subscribe(_ callback: @escaping @Sendable (ProtectedDataEvent) -> Void) {
    lock.lock()
    self.callback = callback
    lock.unlock()
    emit(currentState)
  }

  private func emit(_ state: ProtectedDataState) {
    let event = ProtectedDataEvent(
      state: state,
      observedAtMs: Int64((Date().timeIntervalSince1970 * 1_000).rounded()))
    lock.lock()
    let callback = callback
    lock.unlock()
    callback?(event)
  }
}
