// The iPhone's contacts, for suggestions in the composer's address fields
// (as the desktop uses the Mac's Contacts).
//
// iOS asks once, the first time you type an address; if you say no (or
// share only some contacts), suggestions come from mail you've had alone.
// Names and addresses are read on the phone and never leave it.

import Contacts
import Foundation

actor PhoneContacts {
    static let shared = PhoneContacts()

    /// Everyone with an email address: loaded once, then searched in memory.
    private var people: [ContactSuggestion]?

    /// Contacts whose name or address starts with `prefix` (any word of the
    /// name), best guess first.
    func search(_ prefix: String) async -> [ContactSuggestion] {
        let p = prefix.trimmingCharacters(in: .whitespaces).lowercased()
        guard p.count >= 2, let people = await load() else { return [] }
        return people.filter { c in
            c.email.lowercased().hasPrefix(p) || c.name.lowercased().split(separator: " ").contains { $0.hasPrefix(p) } || c.name.lowercased().hasPrefix(p)
        }
        .prefix(8).map { $0 }
    }

    private func load() async -> [ContactSuggestion]? {
        if let people { return people }
        let store = CNContactStore()
        switch CNContactStore.authorizationStatus(for: .contacts) {
        case .notDetermined:
            guard (try? await store.requestAccess(for: .contacts)) == true else { return nil }
        case .denied, .restricted:
            return nil
        default:
            break
        }
        let keys: [CNKeyDescriptor] = [CNContactGivenNameKey as CNKeyDescriptor, CNContactFamilyNameKey as CNKeyDescriptor,
                                       CNContactOrganizationNameKey as CNKeyDescriptor, CNContactEmailAddressesKey as CNKeyDescriptor]
        var found: [ContactSuggestion] = []
        let request = CNContactFetchRequest(keysToFetch: keys)
        request.sortOrder = .userDefault
        do {
            try store.enumerateContacts(with: request) { c, _ in
                let name = [c.givenName, c.familyName].filter { !$0.isEmpty }.joined(separator: " ")
                for e in c.emailAddresses {
                    found.append(ContactSuggestion(name: name.isEmpty ? c.organizationName : name, email: e.value as String))
                }
            }
        } catch {
            return nil
        }
        people = found
        return found
    }
}
