import Foundation

public enum MarkerScannerError: Error, Equatable {
    case invalidRegex(String)
}

public struct MarkerScanner {
    private struct MarkerPattern {
        var family: MarkerFamily
        var regex: NSRegularExpression
        var extractFields: (String, NSTextCheckingResult) -> [String: MarkerFieldValue]
    }

    public init() {}

    public func scan(data: Data) throws -> SerialIndex {
        let bytes = [UInt8](data)
        let patterns = try Self.makePatterns()
        var lines: [SerialLine] = []
        var lineStart = 0
        var lineNumber = 1

        for index in bytes.indices where bytes[index] == 0x0A {
            lines.append(scanLine(
                bytes: bytes,
                lineStart: lineStart,
                lineEnd: index,
                lineNumber: lineNumber,
                patterns: patterns
            ))
            lineStart = index + 1
            lineNumber += 1
        }

        if lineStart < bytes.count {
            lines.append(scanLine(
                bytes: bytes,
                lineStart: lineStart,
                lineEnd: bytes.count,
                lineNumber: lineNumber,
                patterns: patterns
            ))
        }

        return SerialIndex(byteCount: bytes.count, lines: lines)
    }

    public func scanFile(at url: URL) throws -> SerialIndex {
        try scan(data: Data(contentsOf: url))
    }

    private func scanLine(
        bytes: [UInt8],
        lineStart: Int,
        lineEnd: Int,
        lineNumber: Int,
        patterns: [MarkerPattern]
    ) -> SerialLine {
        var storageEnd = lineEnd
        if storageEnd > lineStart, bytes[storageEnd - 1] == 0x0D {
            storageEnd -= 1
        }

        let lineBytes = Array(bytes[lineStart..<storageEnd])
        let text = String(decoding: lineBytes, as: UTF8.self)
        let lineRange = SerialByteRange(offset: lineStart, length: storageEnd - lineStart)
        let nsText = text as NSString
        let searchRange = NSRange(location: 0, length: nsText.length)
        var hits: [MarkerHit] = []

        for pattern in patterns {
            let matches = pattern.regex.matches(in: text, options: [], range: searchRange)
            var byteSearchStart = 0
            for match in matches {
                let matchedText = nsText.substring(with: match.range)
                let range = Self.byteRange(
                    for: matchedText,
                    in: lineBytes,
                    lineStart: lineStart,
                    startingAt: byteSearchStart,
                    fallbackOffset: match.range.location
                )
                byteSearchStart = min(lineBytes.count, range.endOffset - lineStart)
                hits.append(MarkerHit(
                    family: pattern.family,
                    range: range,
                    fields: pattern.extractFields(text, match),
                    lineNumber: lineNumber
                ))
            }
        }

        hits = Self.dropDuplicateGenericOracleHits(hits)
        hits.sort {
            if $0.range.offset == $1.range.offset {
                return $0.family.rawValue < $1.family.rawValue
            }
            return $0.range.offset < $1.range.offset
        }

        return SerialLine(lineNumber: lineNumber, range: lineRange, text: text, hits: hits)
    }

    private static func byteRange(
        for matchedText: String,
        in lineBytes: [UInt8],
        lineStart: Int,
        startingAt: Int,
        fallbackOffset: Int
    ) -> SerialByteRange {
        let needle = [UInt8](matchedText.utf8)
        if !needle.isEmpty, needle.count <= lineBytes.count, startingAt <= lineBytes.count - needle.count {
            for start in startingAt...(lineBytes.count - needle.count) where Array(lineBytes[start..<(start + needle.count)]) == needle {
                return SerialByteRange(offset: lineStart + start, length: needle.count)
            }
        }
        return SerialByteRange(offset: lineStart + fallbackOffset, length: needle.count)
    }

    private static func dropDuplicateGenericOracleHits(_ hits: [MarkerHit]) -> [MarkerHit] {
        let specificRanges = Set(hits.compactMap { hit in
            hit.family == .oracleGeneric ? nil : hit.range
        })
        return hits.filter { hit in
            hit.family != .oracleGeneric || !specificRanges.contains(hit.range)
        }
    }

    private static func makePatterns() throws -> [MarkerPattern] {
        try [
            // Source: kernel/src/main_aarch64.rs (87 [boot] sites, e.g. :539, :547, :583, :950).
            pattern(.bootStageAarch64, #"\[boot\] (.+)"#) { text, match in
                ["text": .string(group(1, in: text, match: match))]
            },
            // Source: kernel/src/main_aarch64.rs:486-488.
            pattern(.bootBannerAarch64, #"Breenix ARM64 Kernel Starting"#) { _, _ in
                ["event": .string("start")]
            },
            // Source: kernel/src/main_aarch64.rs:486-488; kernel/build.rs:28-33.
            pattern(.bootBannerAarch64, #"BUILD_ID: ([0-9a-f]{14})"#) { text, match in
                ["buildID": .string(group(1, in: text, match: match))]
            },
            // Source: kernel/src/logger.rs:1030-1046.
            pattern(.kernelLogX86, #"(?:(\d+) - )?\[([A-Z ]{5})\] ([^:]+): (.*)"#) { text, match in
                var fields: [String: MarkerFieldValue] = [
                    "level": .string(group(2, in: text, match: match).trimmingCharacters(in: .whitespaces)),
                    "target": .string(group(3, in: text, match: match)),
                    "msg": .string(group(4, in: text, match: match))
                ]
                if let ts = intGroup(1, in: text, match: match) {
                    fields["ts"] = .int(ts)
                }
                return fields
            },
            // Source: kernel/src/test_framework/executor.rs:766,782,790,794,799,804.
            pattern(.testCase, #"\[TEST:([^:\]]+):([^:\]]+):(START|PASS|TIMEOUT|PANIC|FAIL:[^\]]*|DEFERRED:#\d+)\]"#) { text, match in
                let rawState = group(3, in: text, match: match)
                var fields: [String: MarkerFieldValue] = [
                    "suite": .string(group(1, in: text, match: match)),
                    "name": .string(group(2, in: text, match: match))
                ]
                if let detailStart = rawState.firstIndex(of: ":") {
                    fields["state"] = .string(String(rawState[..<detailStart]))
                    fields["detail"] = .string(String(rawState[rawState.index(after: detailStart)...]))
                } else {
                    fields["state"] = .string(rawState)
                }
                return fields
            },
            // Source: kernel/src/test_framework/executor.rs:270,273,276,312.
            pattern(.testComplete, #"\[TESTS_COMPLETE:(\d+)/(\d+)(?::VACUOUS)?(?::FAILED:(\d+))?\]"#) { text, match in
                var fields: [String: MarkerFieldValue] = [
                    "completed": .int(intGroup(1, in: text, match: match) ?? 0),
                    "total": .int(intGroup(2, in: text, match: match) ?? 0),
                    "vacuous": .bool((group(0, in: text, match: match) as NSString).contains(":VACUOUS"))
                ]
                if let failed = intGroup(3, in: text, match: match) {
                    fields["failed"] = .int(failed)
                }
                return fields
            },
            // Source: kernel/src/test_framework/executor.rs:271,274,278,296,311,328-333.
            pattern(.testBootTests, #"\[BOOT_TESTS:(START|PASS|SKIP|TOTAL:\d+|SERIAL_BOOT:\d+|EARLY_BOOT:\d+|STAGED:[^\]]*|FAIL:[^\]]*)\]"#) { text, match in
                let rawState = group(1, in: text, match: match)
                var fields: [String: MarkerFieldValue] = [:]
                if let detailStart = rawState.firstIndex(of: ":") {
                    fields["state"] = .string(String(rawState[..<detailStart]))
                    fields["detail"] = typed(String(rawState[rawState.index(after: detailStart)...]))
                } else {
                    fields["state"] = .string(rawState)
                }
                return fields
            },
            // Source: kernel/src/test_framework/ktap.rs:22-54.
            pattern(.testKTAP, #"^(?:not )?ok (\d+) (.+?)(?: # (SKIP|TIMEOUT))?$"#) { text, match in
                var fields: [String: MarkerFieldValue] = [
                    "num": .int(intGroup(1, in: text, match: match) ?? 0),
                    "name": .string(group(2, in: text, match: match))
                ]
                if !group(3, in: text, match: match).isEmpty {
                    fields["disposition"] = .string(group(3, in: text, match: match))
                }
                return fields
            },
            // Source: kernel/src/test_framework/ktap.rs:22-54.
            pattern(.testKTAP, #"KTAP version 1"#) { _, _ in
                ["version": .int(1)]
            },
            // Source: kernel/src/test_framework/ktap.rs:22-54.
            pattern(.testKTAP, #"1\.\.(\d+)"#) { text, match in
                ["planTotal": .int(intGroup(1, in: text, match: match) ?? 0)]
            },
            // Source: kernel/src/test_framework/btrt.rs:205-209.
            pattern(.testBTRT, #"\[btrt\] Boot Test Result Table at phys (0x[0-9a-f]+) \((\d+) bytes\)"#) { text, match in
                [
                    "phys": .string(group(1, in: text, match: match)),
                    "size": .int(intGroup(2, in: text, match: match) ?? 0)
                ]
            },
            // Source: kernel/src/test_framework/btrt.rs:329.
            pattern(.testBTRT, #"===BTRT_READY==="#) { _, _ in
                ["ready": .bool(true)]
            },
            // Source: userspace/programs/src/heartbeat.rs:168.
            pattern(.heartbeat, #"\[heartbeat\] tid=(\d+) uptime_ms=(\d+) kbd_nonzero=(\d+)"#) { text, match in
                [
                    "tid": .int(intGroup(1, in: text, match: match) ?? 0),
                    "uptimeMs": .int(intGroup(2, in: text, match: match) ?? 0),
                    "kbd": .int(intGroup(3, in: text, match: match) ?? 0)
                ]
            },
            // Source: userspace/programs/src/exec_smoke.rs:11,20; exec_smoke_target.rs:13,18,34; init.rs:285,288.
            pattern(.execSmoke, #"\[EXEC_SMOKE:([A-Z_]+)(?: ([^\]]*))?\]"#) { text, match in
                var fields: [String: MarkerFieldValue] = [
                    "state": .string(group(1, in: text, match: match))
                ]
                if !group(2, in: text, match: match).isEmpty {
                    fields["detail"] = .string(group(2, in: text, match: match))
                }
                return fields
            },
            // Source: generic fallback shape shared across oracle/census rows in DESIGN.md §4.2.
            pattern(.oracleGeneric, #"\[([A-Z][A-Z0-9_]*(?:_ORACLE|_CENSUS)):([^\]]*)\]"#) { text, match in
                var fields = payloadFields(group(2, in: text, match: match))
                relabelArchField(&fields)
                fields["name"] = .string(group(1, in: text, match: match))
                return fields
            },
            // Source: gate literal, docker/qemu/run-aarch64-boot-test-strict.sh:60; run-x86-boot-tests.sh:129.
            pattern(.oracleFutexHandoff, #"\[FUTEX_HANDOFF_ORACLE:aarch64:driven=2:stage1_ret=EAGAIN:stage1_wake=0:stage1_parked=0:stage2_ret=0:stage2_wake=1:stage2_parked=0:stage3_ret=ETIMEDOUT:stage3_elapsed_ok=1:stage3_elapsed_ms=[0-9]+:arm_delay_us=[0-9]+:rescues=0:queue_residual=0:balance=0\]"#) { text, match in
                bracketPayloadFields(group(0, in: text, match: match), prefix: "FUTEX_HANDOFF_ORACLE")
            },
            // Source: gate literal, docker/qemu/run-aarch64-boot-test-strict.sh:85.
            pattern(.oracleFcntlPM, #"\[FCNTL_PM_CONTENTION_ORACLE:aarch64:attempts=[1-3]:armed=1:holder_cpu=[0-9]+:pm_busy_probe=1:calls=64:eagain=0:first_errno=9:first_wait_us=[1-9][0-9]{3,}:hold_done=1:joined=1:PASS\]"#) { text, match in
                bracketPayloadFields(group(0, in: text, match: match), prefix: "FCNTL_PM_CONTENTION_ORACLE")
            },
            // Source: gate literal, docker/qemu/run-aarch64-boot-test-strict.sh:118.
            pattern(.oracleIRQHold, #"\[IRQ_HOLD_ORACLE:aarch64:attempts=[1-3]:armed=1:holder_cpu=[0-9]+:irqs_enabled_before=1:masked_in_hold=1:sends=[1-9][0-9]*:hold_us=[1-9][0-9]{3,}:netrx_pending_at_release=1:received=[1-9][0-9]*:stalled=0:hold_done=1:joined=1:PASS\]"#) { text, match in
                bracketPayloadFields(group(0, in: text, match: match), prefix: "IRQ_HOLD_ORACLE")
            },
            // Source: docker/qemu/run-aarch64-boot-test-strict.sh:340-361.
            pattern(.oraclePollTCP, #"\[POLL_TCP_ORACLE:[^\]]*\]|\[POLL_TCP_TIMEOUT\]|\[POLL_TCP_READY_LOST\]"#) { text, match in
                let marker = group(0, in: text, match: match)
                if marker == "[POLL_TCP_TIMEOUT]" {
                    return ["state": .string("TIMEOUT")]
                }
                if marker == "[POLL_TCP_READY_LOST]" {
                    return ["state": .string("READY_LOST")]
                }
                return bracketPayloadFields(marker, prefix: "POLL_TCP_ORACLE")
            },
            // Source: gate literal, docker/qemu/run-x86-boot-tests.sh:372.
            pattern(.oracleTimerScale, #"\[TIMER_SCALE_ORACLE:x86:ms_per_tick=5:ticks_before=[1-9][0-9]*:ms=[1-9][0-9]*:ticks_after=[0-9]+:ticks_nonzero=1:in_range=1:PASS\]"#) { text, match in
                bracketPayloadFields(group(0, in: text, match: match), prefix: "TIMER_SCALE_ORACLE")
            },
            // Source: docker/qemu/run-aarch64-boot-test-strict.sh:147.
            pattern(.censusTTBR0ASID, #"\[TTBR0_ASID_CENSUS:untagged=(\d+):tagged=(\d+):kernel=(\d+):cleared=(\d+)\]"#) { text, match in
                [
                    "untagged": .int(intGroup(1, in: text, match: match) ?? 0),
                    "tagged": .int(intGroup(2, in: text, match: match) ?? 0),
                    "kernel": .int(intGroup(3, in: text, match: match) ?? 0),
                    "cleared": .int(intGroup(4, in: text, match: match) ?? 0)
                ]
            },
            // Source: docker/qemu/run-aarch64-boot-test-strict.sh:162-164.
            pattern(.censusPinnedHome, #"\[PINNED_HOME_CPU_UNAVAILABLE:(?:count=[0-9]+:publish_discarded=[0-9]+:hold_pen_migrated=[0-9]+:delivered=[0-9]+|first:[^\]]*)\]"#) { text, match in
                bracketPayloadFields(group(0, in: text, match: match), prefix: "PINNED_HOME_CPU_UNAVAILABLE")
            },
            // Source: gate literal, docker/qemu/run-aarch64-boot-test-strict.sh:62.
            pattern(.censusStrand, #"\[SCHED_STRAND_ORACLE:aarch64:samples=[1-9][0-9]*:checked=[1-9][0-9]*:stranded=0:running_shape=[0-9]+:ready_shape=[0-9]+:resolved_production=[0-9]+:resolved_exercised=[1-9][0-9]*:worst_dwell_ms=[0-9]+:overflow=[0-9]+:worst_nonprogress_ms=[0-9]+:nonprogress=[0-9]+:queued_on_nondispatching_cpu=[0-9]+:worst_queued_nondispatch_ms=[0-9]+:worst_cpu_scheduler_silence_ms=[0-9]+:worst_silence_cpu=[0-9]+\]"#) { text, match in
                bracketPayloadFields(group(0, in: text, match: match), prefix: "SCHED_STRAND_ORACLE")
            },
            // Source: gate literal, docker/qemu/run-aarch64-boot-test-strict.sh:63.
            pattern(.censusStrand, #"\[STRAND_INJECT_ORACLE:aarch64:legA_exercised=1:legA_recovered=1:legB_exercised=1:legB_recovered=1:stranded=0\]"#) { text, match in
                bracketPayloadFields(group(0, in: text, match: match), prefix: "STRAND_INJECT_ORACLE")
            },
            // Source: gate literal, docker/qemu/run-aarch64-boot-test-strict.sh:135.
            pattern(.censusStrand, #"\[CENSUS_WIDEN_ORACLE:aarch64:arm_target=[0-9]+:baseline_reported=0:armed_reported=1:tid=[1-9][0-9]*:shape=ready_queued_nondispatching:queued_nondispatching=[1-9][0-9]*:queued_nondispatch_ms=[1-9][0-9]*:cpu_silence_ms=[1-9][0-9]*:joined=1:retired=[01]:PASS\]"#) { text, match in
                bracketPayloadFields(group(0, in: text, match: match), prefix: "CENSUS_WIDEN_ORACLE")
            },
            // Source: kernel/src/arch_impl/aarch64/exception.rs:266-290.
            pattern(.faultEL1First, #"\[UNHANDLED_EC\] cpu=(\d+) EC=(0x[0-9a-f]+) ELR=(0x[0-9a-f]+)"#) { text, match in
                [
                    "kind": .string("UNHANDLED_EC"),
                    "cpu": .int(intGroup(1, in: text, match: match) ?? 0),
                    "ec": .string(group(2, in: text, match: match)),
                    "elr": .string(group(3, in: text, match: match))
                ]
            },
            // Source: kernel/src/arch_impl/aarch64/exception.rs:266-290.
            pattern(.faultEL1First, #"\[EL1_FIRST_FAULT\] instruction_word=(\S+).*"#) { text, match in
                [
                    "kind": .string("EL1_FIRST_FAULT"),
                    "instructionWord": .string(group(1, in: text, match: match))
                ]
            },
            // Source: docker/qemu/run-aarch64-testing-profile-boot-test.sh:97-98.
            pattern(.faultAbort, #"\[(DATA|INSTRUCTION)_ABORT\].*from_el0=([01])"#) { text, match in
                [
                    "kind": .string(group(1, in: text, match: match)),
                    "fromEL0": .bool(group(2, in: text, match: match) == "1")
                ]
            },
            // Source: docker/qemu/run-aarch64-testing-profile-boot-test.sh:66.
            pattern(.faultPanic, #"panicked at kernel/src/"#) { _, _ in
                ["scope": .string("kernel")]
            },
            // Source: docker/qemu/run-aarch64-testing-profile-boot-test.sh:96.
            pattern(.faultPanic, #"thread '.*' panicked at "#) { _, _ in
                ["scope": .string("userspace")]
            },
            // Source: docker/qemu/run-aarch64-testing-profile-boot-test.sh:63.
            pattern(.faultSoftLockup, #"!!! SOFT LOCKUP DETECTED !!!"#) { _, _ in
                [:]
            },
            // Source: docker/qemu/run-aarch64-testing-profile-boot-test.sh:64.
            pattern(.faultExt2Stall, #"EXT2_LOCK_SPIN_STALL"#) { _, _ in
                [:]
            },
            // Source: docker/qemu/run-aarch64-boot-test-strict.sh:265,271.
            pattern(.lockOrder, #"\[(EXEC|CREATION)_LOCK_ORDER:VIOLATION"#) { text, match in
                ["which": .string(group(1, in: text, match: match))]
            },
            // Source: docker/qemu/run-x86-gate.sh:202-203.
            pattern(.devicePCICensus, #"PCI: Enumeration complete\. Found (\d+) devices \((\d+) VirtIO block, (\d+) network\)"#) { text, match in
                [
                    "devices": .int(intGroup(1, in: text, match: match) ?? 0),
                    "virtioBlock": .int(intGroup(2, in: text, match: match) ?? 0),
                    "network": .int(intGroup(3, in: text, match: match) ?? 0)
                ]
            },
            // Source: kernel/src/arch_impl/aarch64/timer_interrupt.rs:516-523.
            pattern(.traceNoise, #"(?<![A-Za-z0-9])(?:T[0-9])+(?![A-Za-z0-9])"#) { text, match in
                ["text": .string(group(0, in: text, match: match))]
            }
        ]
    }

    private static func pattern(
        _ family: MarkerFamily,
        _ regex: String,
        extractFields: @escaping (String, NSTextCheckingResult) -> [String: MarkerFieldValue]
    ) throws -> MarkerPattern {
        do {
            return MarkerPattern(
                family: family,
                regex: try NSRegularExpression(pattern: regex),
                extractFields: extractFields
            )
        } catch {
            throw MarkerScannerError.invalidRegex(regex)
        }
    }

    private static func group(_ index: Int, in text: String, match: NSTextCheckingResult) -> String {
        guard index < match.numberOfRanges else {
            return ""
        }
        let range = match.range(at: index)
        guard range.location != NSNotFound, let swiftRange = Range(range, in: text) else {
            return ""
        }
        return String(text[swiftRange])
    }

    private static func intGroup(_ index: Int, in text: String, match: NSTextCheckingResult) -> Int? {
        Int(group(index, in: text, match: match))
    }

    private static func typed(_ raw: String) -> MarkerFieldValue {
        if let int = Int(raw) {
            return .int(int)
        }
        if raw == "true" {
            return .bool(true)
        }
        if raw == "false" {
            return .bool(false)
        }
        return .string(raw)
    }

    private static func payloadFields(_ payload: String) -> [String: MarkerFieldValue] {
        var fields: [String: MarkerFieldValue] = [:]
        let parts = payload.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
        for (index, part) in parts.enumerated() {
            if let equals = part.firstIndex(of: "=") {
                let key = String(part[..<equals])
                let value = String(part[part.index(after: equals)...])
                fields[key] = typed(value)
            } else if index == 0 {
                fields["state"] = typed(part)
            } else {
                fields["part\(index)"] = typed(part)
            }
        }
        return fields
    }

    private static func bracketPayloadFields(_ marker: String, prefix: String) -> [String: MarkerFieldValue] {
        let head = "[\(prefix):"
        guard marker.hasPrefix(head), marker.hasSuffix("]") else {
            return [:]
        }
        let start = marker.index(marker.startIndex, offsetBy: head.count)
        let end = marker.index(before: marker.endIndex)
        var fields = payloadFields(String(marker[start..<end]))
        fields["name"] = .string(prefix)
        relabelArchField(&fields)
        return fields
    }

    private static func relabelArchField(_ fields: inout [String: MarkerFieldValue]) {
        if case .string(let arch)? = fields["state"], arch == "aarch64" || arch == "x86" {
            fields["arch"] = .string(arch)
            fields.removeValue(forKey: "state")
        }
    }
}
