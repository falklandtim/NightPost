// Small shared pieces: sender pictures, account colours, dates.

import SwiftUI

/// A sender's initials on a colour picked from their address, so the same
/// person always gets the same colour.
struct Avatar: View {
    let name: String
    let email: String
    var size: CGFloat = 40

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .overlay {
                Text(initials)
                    .font(.system(size: size * 0.4, weight: .semibold))
                    .foregroundStyle(.white)
            }
    }

    private var initials: String {
        let first = name.split(separator: ",").first.map(String.init) ?? name
        let words = first.split(separator: " ").filter { $0.first?.isLetter == true }
        let letters = words.prefix(2).compactMap { $0.first }.map { String($0).uppercased() }
        if letters.isEmpty {
            return email.first.map { String($0).uppercased() } ?? "?"
        }
        return letters.joined()
    }

    private var color: Color {
        // A stable hash: Swift's own `hashValue` changes from run to run.
        let n = email.lowercased().unicodeScalars.reduce(UInt32(5381)) { ($0 &* 33) &+ $1.value }
        return Color(hue: Double(n % 360) / 360, saturation: 0.45, brightness: 0.72)
    }
}

extension Color {
    /// Pinned rows are washed with this violet, and their pin drawn in it:
    /// the desktop's PINNED_TINT (`crates/nightpost/src/app/list.rs`), the
    /// same in every theme.
    static let pinnedTint = Color(hex: "8b5cf6")

    /// A pinned row's background, as strong as the desktop's: a pale wash,
    /// deeper when the conversation has unread mail (pinned rows get no
    /// other unread wash, so new mail would show only as the dot).
    static func pinnedWash(dark: Bool, unread: Bool) -> Color {
        let strength: Double = switch (dark, unread) {
        case (true, false): 0.08
        case (false, false): 0.10
        case (true, true): 0.18
        case (false, true): 0.24
        }
        return pinnedTint.opacity(strength)
    }

    /// `#rrggbb`, as the core stores account colours; grey if unreadable.
    init(hex: String) {
        let s = hex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        guard s.count == 6, let v = UInt32(s, radix: 16) else {
            self = .gray
            return
        }
        self.init(red: Double((v >> 16) & 0xff) / 255, green: Double((v >> 8) & 0xff) / 255, blue: Double(v & 0xff) / 255)
    }
}

/// Unix seconds as the list shows them: the time today, the weekday this
/// week, otherwise the date.
func shortDate(_ unix: Int64, now: Date = .now) -> String {
    let d = Date(timeIntervalSince1970: TimeInterval(unix))
    let cal = Calendar.current
    if cal.isDate(d, inSameDayAs: now) {
        return d.formatted(date: .omitted, time: .shortened)
    }
    if let days = cal.dateComponents([.day], from: d, to: now).day, abs(days) < 7 {
        return d.formatted(.dateTime.weekday(.abbreviated))
    }
    if cal.component(.year, from: d) == cal.component(.year, from: now) {
        return d.formatted(.dateTime.day().month(.abbreviated))
    }
    return d.formatted(date: .numeric, time: .omitted)
}
