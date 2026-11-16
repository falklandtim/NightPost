// When to bring a conversation back: the desktop's quick choices, or a
// date and time.

import SwiftUI

struct SnoozeSheet: View {
    let pick: (Date) -> Void
    @State private var custom = Calendar.current.date(byAdding: .day, value: 1, to: .now) ?? .now
    @State private var choosing = false

    var body: some View {
        NavigationStack {
            List {
                if choosing {
                    DatePicker("Until", selection: $custom, in: Date.now..., displayedComponents: [.date, .hourAndMinute])
                        .datePickerStyle(.graphical)
                    Button("Snooze") { pick(custom) }
                } else {
                    ForEach(SnoozeChoice.all(now: .now), id: \.title) { c in
                        Button {
                            pick(c.date)
                        } label: {
                            HStack {
                                Label(c.title, systemImage: c.icon)
                                Spacer()
                                Text(c.date, format: .dateTime.weekday(.abbreviated).hour().minute())
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    Button { choosing = true } label: { Label("Pick a date…", systemImage: "calendar") }
                }
            }
            .navigationTitle("Snooze until")
            .navigationBarTitleDisplayMode(.inline)
        }
    }
}

/// A quick snooze time.
struct SnoozeChoice {
    let title: String
    let icon: String
    let date: Date

    /// Later today (3 hours), this evening, tomorrow morning, this weekend
    /// and next week, as on the desktop; mornings are 8:00.
    static func all(now: Date, calendar: Calendar = .current) -> [SnoozeChoice] {
        func at(_ day: Date, _ hour: Int) -> Date {
            calendar.date(bySettingHour: hour, minute: 0, second: 0, of: day) ?? day
        }
        let today = calendar.startOfDay(for: now)
        let tomorrow = calendar.date(byAdding: .day, value: 1, to: today) ?? today
        let later = now.addingTimeInterval(3 * 3600)
        var out: [SnoozeChoice] = [SnoozeChoice(title: "Later today", icon: "clock", date: later)]
        // Only when it's clearly after "Later today", so the list stays in
        // time order (in the afternoon, three hours on is already evening).
        let evening = at(today, 18)
        if evening > later.addingTimeInterval(3600) {
            out.append(SnoozeChoice(title: "This evening", icon: "moon", date: evening))
        }
        out.append(SnoozeChoice(title: "Tomorrow", icon: "sunrise", date: at(tomorrow, 8)))
        let weekday = calendar.component(.weekday, from: now) // 1 = Sunday
        if weekday != 7 && weekday != 1 {
            let saturday = calendar.date(byAdding: .day, value: 7 - weekday, to: today) ?? today
            out.append(SnoozeChoice(title: "This weekend", icon: "sofa", date: at(saturday, 8)))
        }
        let toMonday = (9 - weekday) % 7 == 0 ? 7 : (9 - weekday) % 7
        let monday = calendar.date(byAdding: .day, value: toMonday, to: today) ?? today
        out.append(SnoozeChoice(title: "Next week", icon: "calendar", date: at(monday, 8)))
        return out
    }
}
