/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import Foundation

import Mentatlib

protocol Observing {
    // define functions for store observation
    func transactionDidOccur(key: String, reports: [TxReport])
}

protocol Observable {
    func register(key: String, observer: Observing, attributes: [String])
    func unregister(key: String)
}

class Mentat: RustObject {
    fileprivate static var observers = [String: Observing]()

    required override init(raw: OpaquePointer) {
        super.init(raw: raw)
    }

    convenience init(storeURI: String = "") {
        self.init(raw: store_open(storeURI))
    }


    func entidForAttribute(attribute: String) -> Int64 {
        return Int64(store_entid_for_attribute(self.raw, attribute))
    }

    func sync_now() -> Bool {
        let err = store_sync(self.raw, "00000000-0000-0000-0000-000000000117", "http://mentat.dev.lcip.org/mentatsync/0.1")
        if let error = err.pointee.err {
            let str = String(cString: error)
            print("Sync error \(str)")
            return false
        }

        return true
    }

    func query(query: String) -> Query {
        return Query(raw: store_query(self.raw, query))
    }

    func value(forAttribute attribute: String, ofEntity entid: Int64) -> TypedValue? {
        let result = store_value_for_attribute(self.raw, entid, attribute).pointee
        guard let success = result.ok else {
            if let error = result.err {
                let str = String(cString: error)
                print("Error: \(str)")
            }
            return nil
        }
        return TypedValue(raw: success)
    }

    override func cleanup(pointer: OpaquePointer) {
        store_destroy(pointer)
    }
}

extension Mentat: Observable {
    func register(key: String, observer: Observing, attributes: [String]) {
        let attrEntIds = attributes.map({ (kw) -> Int64 in
            let entid = Int64(self.entidForAttribute(attribute: kw));
            return entid
        })

        let ptr = UnsafeMutablePointer<Int64>.allocate(capacity: attrEntIds.count)
        let entidPointer = UnsafeMutableBufferPointer(start: ptr, count: attrEntIds.count)
        var _ = entidPointer.initialize(from: attrEntIds)

        guard let firstElement = entidPointer.baseAddress else {
            return
        }
        Mentat.observers[key] = observer
        store_register_observer(self.raw, key, firstElement, Int64(attributes.count), transactionObserverCallback)

    }

    func unregister(key: String) {
        store_unregister_observer(self.raw, key)
    }
}

private func transactionObserverCallback(key: UnsafePointer<CChar>, reports: UnsafePointer<TxReportList>) {
    // needs to be done in the same thread as the calling thread otherwise the TxReportList might be released before
    // we can reference it.
    let key = String(cString: key)
    guard let observer = Mentat.observers[key] else { return }

    let len = Int(reports.pointee.len)
    var txReports = [TxReport]()
    for i in 0..<len {
        let raw = tx_report_list_entry_at(reports, i)
        let report = TxReport(raw: raw!)
        txReports.append(report)
    }
    DispatchQueue.global(qos: .background).async {
        observer.transactionDidOccur(key: key, reports: txReports)
    }
}