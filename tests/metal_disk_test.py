"""Physical-root delivery and cold raw readback; no controller is contacted."""
import hashlib
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing
import metal_disk as disk
import metal_root


class DiskTest(existing.StateTest):
    def setUp(self):
        super().setUp()
        self.directory = existing.m.STATE / 'boots' / 'suite'
        self.directory.mkdir(parents=True)
        self.inputs = self.directory / 'inputs'
        self.inputs.mkdir()
        image = bytearray(4096)
        image[1080:1082] = b'\x53\xef'
        (self.inputs / 'ext2.img').write_bytes(image)
        (self.inputs / 'stub.efi').write_bytes(b'stub')
        self.build = {'ext2': {'sha256': hashlib.sha256(image).hexdigest()}}
        self.site = {'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture_password_only_12345',
                     'CONSOLE_IDENTITY_KEY': 'ab' * 32}
        existing.m.write_json(existing.m.STATE / 'disk.json',
            dict(installedBy='prior', bytes=4096, sha256AfterRecord='a' * 64))

    def stage(self, boot='first'):
        return disk.stage(self.directory, self.inputs, boot, 'b' * 40, self.site, self.build)

    def test_breenfs_tools_are_verified_and_staged(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.mode, d.args.disk, d.args.commit, d.args.root_fs = 'default', 'root', 'b' * 40, 'breenfs'
            d.args.ramdisk = True
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            tools = {}
            for name in ('breenfs-mkfs', 'breenfs-check', 'breenfs-extract'):
                path = inputs / name
                path.write_bytes(name.encode())
                tools[name] = dict(size=path.stat().st_size, sha256=hashlib.sha256(path.read_bytes()).hexdigest())
            build = dict(commit=d.args.commit, profile='production', features=[], breenfsTools=tools)
            existing.m.write_json(inputs / 'build.json', build)
            for invalid in ('record', 'entry', 'file', 'size', 'digest'):
                bad = dict(build)
                bad['breenfsTools'] = dict(tools)
                if invalid == 'record':
                    del bad['breenfsTools']
                elif invalid == 'entry':
                    del bad['breenfsTools']['breenfs-mkfs']
                elif invalid == 'file':
                    (inputs / 'breenfs-mkfs').unlink()
                else:
                    bad['breenfsTools']['breenfs-check'] = dict(tools['breenfs-check'])
                    bad['breenfsTools']['breenfs-check'][invalid if invalid == 'size' else 'sha256'] = (1 if invalid == 'size' else '0' * 64)
                existing.m.write_json(inputs / 'build.json', bad)
                with self.assertRaisesRegex(existing.boot.Finish, 'breenfs tools'):
                    d.prepare_payload()
                (inputs / 'breenfs-mkfs').write_bytes(b'breenfs-mkfs')
            existing.m.write_json(inputs / 'build.json', build)
            with patch.object(existing.boot.metal_disk, 'stage', return_value=({}, {})) as stage, \
                 patch.object(existing.boot.metal_image, 'extract_esp', return_value={}), \
                 patch.object(existing.boot.metal_image, 'assemble', side_effect=RuntimeError('captured')):
                with self.assertRaisesRegex(RuntimeError, 'captured'):
                    d.prepare_payload()
            self.assertEqual(stage.call_args.kwargs['root_fs'], 'breenfs')
            self.assertEqual(stage.call_args.kwargs['tools_dir'], inputs)
            d.args.target, d.args.task = 'main', ''
            import shutil
            shutil.rmtree(d.dir)
            d.create_record()
            self.assertEqual(existing.m.read_json(d.dir / 'run.json')['rootFs'], 'breenfs')
        finally:
            fixture.tearDown()

    def test_breenfs_build_records_linux_release_tools_in_both_paths(self):
        import os
        import tempfile
        for local in (False, True):
            with self.subTest(local=local), tempfile.TemporaryDirectory() as temporary:
                build = existing.load_script('metal-build')
                args = existing.SimpleNamespace(id='20261117T000000Z-metal', root=temporary,
                    sha='a' * 40, stub=temporary, suite='', features='', guard_target='x86-metal', local=local)
                clone, out, tmp = build.paths(args)
                out.mkdir(parents=True)
                for path in (clone / 'tools/breenfs/Cargo.toml', clone / 'testdata/ext2.img',
                             clone / 'target/release/build/breenix-0/out/breenix-uefi.img',
                             tmp / 'stub-target' / build.STUB_RELEASE / 'metal-efi-stub.efi',
                             tmp / 'stub-target' / build.STUB_RELEASE / 'metal-boot-manager.efi'):
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(b'fixture')
                calls = []
                names = ('breenfs-mkfs', 'breenfs-check', 'breenfs-extract')
                def call(command, **options):
                    if 'breenfs-mkfs' in command:
                        calls.append((command, options))
                        self.assertIn('--release', command)
                        self.assertEqual(command[command.index('--target') + 1], 'x86_64-unknown-linux-gnu')
                        target = Path(command[command.index('--target-dir') + 1])
                        for name in names:
                            self.assertIn(name, command)
                            binary = target / 'x86_64-unknown-linux-gnu/release' / name
                            binary.parent.mkdir(parents=True, exist_ok=True)
                            binary.write_bytes(name.encode())
                    return 0
                with (patch.dict(os.environ, CARGO_BUILD_JOBS='4', CARGO_HOME=str(Path(temporary) / 'cargo')),
                      patch.object(build.subprocess, 'call', side_effect=call),
                      patch.object(build.subprocess, 'run', return_value=existing.SimpleNamespace(stdout=str(Path(temporary) / 'cargo')))):
                    self.assertEqual(build.inner(args), 0)
                self.assertEqual(len(calls), 1)
                record = json.loads((out / 'build.json').read_text())
                self.assertEqual(set(record['breenfsTools']), set(names))
                for name in names:
                    self.assertEqual((out / name).read_bytes(), name.encode())
                    self.assertEqual(record['breenfsTools'][name],
                        dict(size=len(name), sha256=hashlib.sha256(name.encode()).hexdigest()))
                if local:
                    linker = Path(calls[0][1]['env']['CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER'])
                    self.assertIn('x86_64-linux-gnu', linker.read_text())

    def test_breenfs_verdict_retains_tools_at_the_scorers_default_path(self):
        import os
        import runpy
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.mode, d.args.suite, d.args.root_fs = 'suite', 'files-io', 'breenfs'
            retained = d.dir / 'breenfs-tools'
            retained.mkdir()
            for name in ('breenfs-extract', 'breenfs-check'):
                (retained / name).write_text('#!' + sys.executable + '\nprint("fixture")\n')
            definition = d.dir / 'definition'
            repo = Path(__file__).resolve().parents[1]
            for name in ('scripts/suite-verdict.py', 'docs/boot-path.json',
                         'docs/suites/files-io.json', 'libs/libbreenix/src/suite_memory.rs'):
                path = definition / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes((repo / name).read_bytes())
            (d.dir / 'serial.txt').write_text('serial')
            real_run = existing.subprocess.run
            def score(command, **options):
                self.assertEqual(options['env'], {'PATH': os.defpath, 'LANG': 'C.UTF-8'})
                binding = next(x for x in command if x.startswith('--property=BindReadOnlyPaths='))
                root = Path(binding.split('=', 2)[2].split(':')[0])
                scorer = runpy.run_path(str(root / 'scripts/suite-verdict.py'))
                for name in ('breenfs-extract', 'breenfs-check'):
                    tool = scorer['REPO'] / 'tools/breenfs/target/release' / name
                    result = real_run([str(tool)], capture_output=True, text=True, env=options['env'])
                    self.assertEqual((result.returncode, result.stdout), (0, 'fixture\n'))
                return existing.SimpleNamespace(returncode=0, stdout='pass', stderr='')
            with patch.object(existing.boot.subprocess, 'run', side_effect=score):
                self.assertTrue(d.score_suite())
        finally:
            fixture.tearDown()

    def test_each_boot_reinstalls_even_unchanged_userspace_and_credentials(self):
        original = (self.inputs / 'ext2.img').read_bytes()
        previous = existing.m.read_json(existing.m.STATE / 'disk.json')
        for boot, commit in [('first', 'b' * 40), ('next', 'c' * 40), ('same-commit', 'c' * 40)]:
            (self.inputs / 'ext2.img').write_bytes(original)
            with patch.object(metal_root, 'stage_root') as root:
                facts, files = disk.stage(self.directory, self.inputs, boot, commit, self.site, self.build,
                                         mode='suite', suite='files-io')
                root.assert_called_once()
                self.assertEqual(root.call_args.args[1:3], ('suite', 'files-io'))
            self.assertEqual(set(files), {'suite-disk.cfg', 'suite-root.img'})
            request = disk.HEADER.unpack(files['suite-disk.cfg'].read_bytes())
            self.assertEqual(request[1:3], (1, 4096))
            self.assertEqual(request[3], disk.record(previous))
            self.assertEqual(request[4], disk.record(facts))
            self.assertEqual(facts['installedBy'], boot)
            existing.m.write_json(existing.m.STATE / 'disk.json', facts)
            previous = facts
        # Private credentials are also staged anew, even with unchanged ext2.
        (self.inputs / 'ext2.img').write_bytes(original)
        self.site['CONSOLE_PASSWORD'] += '_changed'
        with patch.object(metal_root, 'stage_root') as root:
            changed, files = self.stage('credentials')
            self.assertEqual(root.call_args.args[3]['CONSOLE_PASSWORD'], self.site['CONSOLE_PASSWORD'])
        self.assertNotEqual(changed['buildDigest'], previous['buildDigest'])
        self.assertIn('suite-root.img', files)

    def test_default_disk_boot_carries_current_boot_intent(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.mode, d.args.disk, d.args.commit = 'default', 'root', 'b' * 40
            d.args.ramdisk = True
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            existing.m.write_json(inputs / 'build.json', dict(commit=d.args.commit, profile='production', features=[]))
            with patch.object(existing.boot.metal_disk, 'stage', return_value=({}, {})) as stage, \
                 patch.object(existing.boot.metal_image, 'extract_esp', return_value={}), \
                 patch.object(existing.boot.metal_image, 'assemble', side_effect=RuntimeError('captured')) as assemble:
                with self.assertRaisesRegex(RuntimeError, 'captured'):
                    d.prepare_payload()
            self.assertEqual(assemble.call_args.args[1]['ramdisk'].read_bytes(), b'BREENIX-BOOT 1\ndefault\n')
            self.assertEqual(stage.call_args.kwargs['mode'], 'default')
        finally:
            fixture.tearDown()

    def test_multi_suite_disk_intent_boots_first_suite_and_scores_cold_disk(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.mode, d.args.disk, d.args.commit = 'suite', 'root', 'b' * 40
            d.args.suite, d.args.ramdisk = 'smoke,files-io', True
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            (inputs / 'stub.efi').write_bytes(b'stub')
            existing.m.write_json(inputs / 'build.json', dict(commit=d.args.commit, profile='production', features=[], suite=d.args.suite))
            definition = inputs / 'definition'
            for name, contents in {'docs/suites/smoke.json': '{"id":"smoke"}',
                                   'docs/suites/files-io.json': '{"id":"files-io","diskChecks":[{}]}',
                                   'scripts/suite-sequence.py': '# scorer', 'scripts/suite-verdict.py': '# verdict',
                                   'docs/boot-path.json': '{}',
                                   'libs/libbreenix/src/suite_memory.rs': (Path(__file__).resolve().parents[1] / 'libs/libbreenix/src/suite_memory.rs').read_text()}.items():
                path = definition / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(contents)
            with patch.object(existing.boot.metal_disk, 'stage', return_value=({}, {})) as stage, \
                 patch.object(existing.boot.metal_image, 'extract_esp', return_value={}), \
                 patch.object(existing.boot.metal_image, 'assemble', side_effect=RuntimeError('captured')) as assemble:
                with self.assertRaisesRegex(RuntimeError, 'captured'):
                    d.prepare_payload()
            self.assertEqual(assemble.call_args.args[1]['ramdisk'].read_bytes(), b'BREENIX-BOOT 1\nsuite smoke hostkeys host-channel\n')
            self.assertEqual(stage.call_args.kwargs['suite'], 'smoke,files-io')
            self.assertEqual((d.dir / 'readback-stub.efi').read_bytes(), b'stub')
            (d.dir / 'serial.txt').write_text('serial')
            raw = d.dir / 'raw.img'
            raw.write_bytes(b'disk')
            with patch.object(existing.boot.subprocess, 'run', return_value=existing.SimpleNamespace(returncode=0, stdout='pass', stderr='')) as scorer:
                self.assertTrue(d.score_suite(disk=raw))
            command = scorer.call_args.args[0]
            self.assertEqual(command[-6:], ['/run/metal-verdict-input/scripts/suite-sequence.py', 'score', 'smoke,files-io',
                                           '/run/metal-verdict-input/serial.txt', '--disk', '/run/metal-verdict-input/disk.img'])
        finally:
            fixture.tearDown()

    def test_pre_mount_fault_replaces_only_the_delivered_bootloader(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.mode, d.args.disk, d.args.commit = 'default', 'root', 'b' * 40
            d.args.inject = 'disk-before-mount'
            d.args.ramdisk = True
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            loader = inputs / 'loader.efi'
            loader.write_bytes(b'original EFI image')
            existing.m.write_json(inputs / 'build.json', dict(commit=d.args.commit, profile='production', features=[]))
            with patch.object(existing.boot.metal_disk, 'stage', return_value=({}, {})), \
                 patch.object(existing.boot.metal_image, 'extract_esp', return_value={'efi/boot/bootx64.efi': loader}), \
                 patch.object(existing.boot.metal_image, 'assemble', side_effect=RuntimeError('captured')):
                with self.assertRaisesRegex(RuntimeError, 'captured'):
                    d.prepare_payload()
            self.assertEqual(loader.read_bytes(), b'injected invalid EFI image\n')
        finally:
            fixture.tearDown()

    def test_install_record_survives_a_kernel_panic_before_mount(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.disk = 'root'
            facts = dict(installedBy='fresh', bytes=4096, sha256AfterRecord='a' * 64)
            d.delivery['diskRoot'] = facts
            d.on_disk_line('metal-stub: suite fixture verified: 4096 bytes', 2)
            self.assertEqual(existing.m.read_json(existing.m.STATE / 'disk.json'), dict(facts, lastBoot=d.id))
            self.assertNotIn('disk-root', d.seen)
            # A short fixture must not replace the ownership record.
            d.seen.discard('disk-fixture')
            with self.assertRaises(existing.boot.Finish):
                d.on_disk_line('metal-stub: suite fixture verified: 2048 bytes', 3)
            self.assertEqual(existing.m.read_json(existing.m.STATE / 'disk.json'), dict(facts, lastBoot=d.id))
        finally:
            fixture.tearDown()

    def test_lease_handoff_keeps_competitors_out_and_preserves_the_live_unit(self):
        m = existing.m
        self.ready()
        m.claim_lease(dict(id='suite', unit='suite-unit'), wait=False)
        self.assertFalse(m.handoff_lease('wrong-owner', dict(id='readback', unit='suite-unit')))
        with self.assertRaises(m.Busy):
            m.claim_lease(dict(id='competitor'), wait=False)
        self.assertTrue(m.handoff_lease('suite', dict(id='readback', unit='suite-unit')))
        self.assertEqual(m.read_lease()['unit'], 'suite-unit')
        self.assertFalse(m.release_lease('suite'))
        with self.assertRaises(m.Busy):
            m.claim_lease(dict(id='competitor'), wait=False)
        m.quarantine_lease('readback', 'power state uncertain')
        self.assertFalse(m.handoff_lease('readback', dict(id='third', unit='suite-unit')))

    def readback_source(self, status, reason, provisional=False, source_id='20261018T000000Z-metal'):
        m = existing.m
        source = existing.m.boot_dir(source_id)
        source.mkdir(parents=True)
        facts = dict(installedBy='prior', bytes=4096, sha256AfterRecord='a' * 64)
        m.write_json(source / 'run.json', dict(disk='root', mode='suite', suite='files-io'))
        m.write_json(source / 'suite-disk.json', facts)
        manifest = source / 'definition/docs/suites/files-io.json'
        manifest.parent.mkdir(parents=True)
        m.write_json(manifest, dict(id='files-io', diskChecks=[dict(path='/written', byte=66, length=4)]))
        (source / 'commit').write_text('b' * 40)
        events = [dict(event='disk-root-record', matchesLastInstall=True),
                  dict(event='decided', status=status, reason=reason),
                  dict(event='power-off', off=True, t=1),
                  dict(event='suite-awaiting-disk-readback', provisional=provisional)]
        (source / 'events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in events))
        m.close_record(source, m.EXIT_SHORT if provisional else status, 'driver')
        m.write_json(m.STATE / 'disk.json', dict(facts, lastBoot=source_id))
        return source, facts

    def test_cold_readback_accepts_deadline_ended_source_and_records_status(self):
        source, _ = self.readback_source(1, 'the mode deadline passed with stages missing')
        self.site['BOOT_URL'] = 'http://example.invalid/boot'
        with patch.object(disk.metal_image, 'assemble', side_effect=lambda image, _: image.touch()), \
             patch.object(disk.metal_image, 'stub_efi', return_value={}):
            disk.prepare_readback(self.directory, self.inputs, source.name, 'b' * 40, 'readback', self.site, self.inputs / 'stub.efi')
        self.assertEqual(existing.m.read_json(self.directory / 'readback.json')['sourceStatus'], 1)

    def test_cold_readback_refuses_an_intervening_boot_even_with_the_same_root(self):
        source, facts = self.readback_source(0, 'suite complete', provisional=True)
        existing.m.write_json(existing.m.STATE / 'disk.json', dict(facts, lastBoot='another-boot'))
        with self.assertRaisesRegex(ValueError, 'replaced between'):
            disk.prepare_readback(self.directory, self.inputs, source.name, 'b' * 40, 'readback', self.site, self.inputs / 'stub.efi')

    def test_cut_readback_and_recovery_admit_only_the_original_test_partition(self):
        m = existing.m
        source, facts = self.readback_source(m.EXIT_SHORT, 'deliberate power cut')
        partition = '12345678-abcd-1234-abcd-000000000123'
        facts.update(installedBy=source.name, partition=partition)
        m.write_json(source / 'suite-disk.json', facts)
        m.write_json(source / 'run.json', dict(id=source.name, disk='root', mode='suite',
            suite='filesystem', onlyCase='power-cut/writer', rootFs='breenfs',
            powerCut=dict(powerOffConfirmed=True, cutAt=3)))
        m.write_json(m.STATE / 'disk.json', dict(facts, lastBoot=source.name))
        events = [
            dict(event='disk-root-record', matchesLastInstall=True, installedBy=source.name, t=1),
            dict(event='power-cut-writer-start', nonce='0123456789abcdef', t=2),
            dict(event='decided', status=m.EXIT_SHORT, reason='deliberate power cut', t=4),
            dict(event='power-off', off=True, t=5),
        ]
        (source / 'events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in events))
        (source / 'serial.txt').write_text(
            'SUITE filesystem START cases=1 only=power-cut/writer nonce=0123456789abcdef\n'
            + 'FSYNC 0123456789abcdef 1 ' + 'a' * 64 + '\n')
        self.site['BOOT_URL'] = 'http://example.invalid/boot'
        with patch.object(disk.metal_persist, 'layout', return_value={'partitions': {'test': {'guid': partition}}}), \
             patch.object(disk.metal_image, 'assemble', side_effect=lambda image, _: image.touch()), \
             patch.object(disk.metal_image, 'stub_efi', return_value={}):
            disk.prepare_readback(self.directory, self.inputs, source.name, 'b' * 40, 'readback',
                                  self.site, self.inputs / 'stub.efi')
            request = disk.HEADER_2.unpack_from((self.inputs / 'suite-disk.cfg').read_bytes())
            self.assertEqual(request[1], 2)  # Firmware raw read, never stage.
            self.assertEqual(request[3], disk.uuid.UUID(partition).bytes_le)
            self.assertTrue(m.read_json(self.directory / 'readback.json')['powerCut'])
            result = dict(id='readback', passed=True, logBytes=4096, logSha256='e' * 64)
            m.write_json(source / 'cut-readback.json', result)
            self.assertEqual(disk.prepare_cut_recovery(source.name, 'readback', 'b' * 40), (facts, result))
            m.write_json(m.STATE / 'disk.json', dict(facts, lastBoot='another-boot'))
            with self.assertRaisesRegex(ValueError, 'replaced between'):
                disk.prepare_cut_recovery(source.name, 'readback', 'b' * 40)

    def test_recovery_driver_serves_the_retained_payload_without_staging_a_root(self):
        m, boot = existing.m, existing.boot
        source, facts = self.readback_source(m.EXIT_SHORT, 'deliberate power cut')
        retained = source / 'recovery.efi'
        retained.write_bytes(b'recovery fixture')
        m.write_json(source / 'recovery-payload.json',
                     dict(size=retained.stat().st_size, sha256=disk.metal_image.sha256(retained)))
        args = existing.SimpleNamespace(id='20261018T000002Z-metal', mode='suite',
            payload='image', disk='root', suite='filesystem', commit='b' * 40, inject=None,
            case='power-cut/recovery', source_boot=source.name, cut_recovery_from='readback')
        expected = dict(logBytes=4096, logSha256='e' * 64)
        with patch.object(m, 'load_site', return_value={}):
            driver = boot.Driver(args)
        with patch.object(disk, 'prepare_cut_recovery', return_value=(facts, expected)), \
             patch.object(disk, 'stage', side_effect=AssertionError('must not stage a root')), \
             patch.object(driver, 'load_definition'):
            driver.prepare_payload()
        self.assertEqual(driver.delivery['diskRoot'], facts)
        self.assertEqual(driver.payload_facts['path'].read_bytes(), retained.read_bytes())
        self.assertEqual(driver.recovery_expected, expected)

    def test_only_decided_failure_schedules_diagnostic_readback(self):
        m, boot = existing.m, existing.boot
        for minute, status, reason in [(0, m.EXIT_STOPPED, 'operator stop'),
                                       (1, m.EXIT_INCOMPLETE, 'console capture incomplete'),
                                       (2, m.EXIT_SHORT, 'a fatal line')]:
            with self.subTest(status=status):
                source, facts = self.readback_source(status, reason, source_id=f'20261018T00{minute:02}00Z-metal')
                args = existing.SimpleNamespace(id=source.name, mode='suite', payload='image', disk='root',
                                                suite='files-io', commit='b' * 40, inject=None)
                with patch.object(m, 'load_site', return_value={}):
                    driver = boot.Driver(args)
                driver.t0 = driver.kernel_started = 2
                driver.delivery['diskRoot'] = facts
                root_line = '[storage] root record: installed by boot prior, 4096 bytes, sha256 ' + 'a' * 64
                driver.on_disk_line(root_line, 2)
                for method in ('create_record', 'claim', 'prepare_payload', 'preflight', 'do_stage',
                               'start_console', 'power_on', 'pump_serial', 'cleanup'):
                    setattr(driver, method, Mock())
                driver.power_off = Mock(return_value=True)
                driver.watch = Mock(side_effect=boot.Finish(status, reason))
                driver.continue_disk_readback = Mock(return_value=m.EXIT_DONE)
                (source / 'definition/docs/metal').mkdir()
                (source / 'definition/docs/metal/boot-path.json').touch()
                with patch.object(boot.metal_score, 'score', return_value={}), \
                     patch.object(boot.metal_score, 'falls_short', return_value=True):
                    self.assertEqual(driver.run(), status)
                if status == m.EXIT_SHORT:
                    driver.continue_disk_readback.assert_called_once()
                else:
                    driver.continue_disk_readback.assert_not_called()

    def test_decided_failure_keeps_its_exit_after_passing_disk_checks(self):
        m, boot = existing.m, existing.boot
        source, _ = self.readback_source(m.EXIT_SHORT, 'a fatal line')
        args = existing.SimpleNamespace(id='20261018T000001Z-metal', mode='default', payload='disk-readback')
        with patch.object(m, 'load_site', return_value={}):
            driver = boot.Driver(args)
            driver.dir.mkdir()
            image = driver.dir / 'disk.img'
            image.write_bytes(b'disk')
            driver.t0 = 2
            sha = hashlib.sha256(b'disk').hexdigest()
            m.write_json(driver.dir / 'readback.json', dict(source=source.name, bytes=4, rootRecord='record',
                suitePoweredOffAt=1, sourceStatus=m.EXIT_SHORT, sourceProvisional=False,
                sourceReason='a fatal line', sourceEndedBy='driver'))
            m.write_json(driver.dir / 'disk-upload.json', dict(complete=True, sha256=sha, rootRecord='record'))
            m.write_json(source / 'milestone-verdict.json', {'fixture': 'passing'})

            verdict = dict(passed=True, detail='raw disk checks passed=1',
                           diskChecks=[dict(path='/written', status='pass')], breenfsCheckExit=0)
            def score_suite(*, disk, disk_report=None):
                self.assertEqual(disk, image)
                if disk_report is not None:
                    disk_report.update(verdict)
                return True

            with patch.object(boot.Driver, 'score_suite', side_effect=score_suite), \
                 patch.object(boot.metal_score, 'falls_short', return_value=False):
                self.assertTrue(driver.finalize_disk_suite())
        self.assertEqual((source / 'exit-status').read_text(), '1\n')
        receipt = m.read_json(source / 'suite-readback.json')
        self.assertEqual(receipt['diskVerdict'], verdict)
        self.assertEqual(receipt['sourceStatus'], m.EXIT_SHORT)
        self.assertEqual(receipt['sourceReason'], 'a fatal line')
        self.assertFalse(receipt['sourceProvisional'])


if __name__ == '__main__':
    unittest.main()
