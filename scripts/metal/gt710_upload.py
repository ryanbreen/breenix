"""Private, bounded actual-display-row upload for the powered boot's lease."""
import hashlib
import json
import intent_manifest
import http.client
import os
from pathlib import Path
import re
import secrets
import time
from urllib.parse import urlsplit, urlunsplit
import metal_lib as m
MAX_BYTES=2<<20
# R6 amended: includes startup, picture chunks and the final evidence slot.
MAX_TOTAL=64<<20
PREFIX='/gt710-upload/'
def crc_checkpoint_cap(suite):
    root = Path(__file__).resolve().parents[2]
    path = root / 'docs/suites/frames/checkpoints.json'
    if not path.exists(): path = Path(__file__).with_name('checkpoints.json')
    checkpoints = json.loads(path.read_text())
    return len(checkpoints[suite]) + intent_manifest.counts()[suite] + 4

def prepare(directory,boot_id,site,suite=None):
    route=PREFIX+secrets.token_hex(32)
    intent=dict(id=boot_id,route=route,suite=suite)
    m.write_json(m.STATE/'gt710-upload.json',intent)
    m.write_json(Path(directory)/'native-upload-intent.json',intent)
    url=urlsplit(site['BOOT_URL'])
    return urlunsplit((url.scheme,url.netloc,route,'',''))

def ready(handler, backend=m):
    """Check the staged route without consuming an artifact slot."""
    with backend.locked('gt710-upload'):
        intent = backend.read_json(backend.STATE / 'gt710-upload.json', {})
        lease = backend.read_lease() or {}
        if (not intent or urlsplit(handler.path).path != intent.get('route')
                or lease.get('id') != intent.get('id')
                or lease.get('state') not in ('claimed', 'powered')
                or (backend.boot_dir(intent['id']) / 'ended').exists()):
            handler.send_error(404)
            return
        handler.send_response(204)
        handler.send_header('X-Breenix-Boot', intent['id'])
        handler.send_header('X-Breenix-Picture-References', '1')
        handler.send_header('Content-Length', '0')
        handler.end_headers()

def check_ready(directory, boot_id, site):
    """Require the live HTTP handler before a native payload is powered on."""
    intent = m.read_json(Path(directory) / 'native-upload-intent.json')
    if not intent:
        return
    if intent.get('id') != boot_id or not intent.get('route', '').startswith(PREFIX):
        raise ValueError('native upload intent differs from this boot')
    url = urlsplit(site['BOOT_URL'])
    port = int(site.get('HTTP_LISTEN_PORT') or url.port or 80)
    deadline = time.monotonic() + 5
    while True:
        connection = http.client.HTTPConnection('127.0.0.1', port, timeout=2)
        try:
            connection.request('HEAD', intent['route'])
            response = connection.getresponse()
            if response.status != 204 or response.getheader('X-Breenix-Boot') != boot_id:
                raise ValueError('native upload route unavailable; deploy tools with --restart')
            if (m.read_json(Path(directory) / 'run.json', {}).get('screenProof') is True
                    and response.getheader('X-Breenix-Picture-References') != '1'):
                raise ValueError('native picture reference receiver unavailable; deploy tools with --restart')
            return
        except (OSError, http.client.HTTPException) as error:
            if time.monotonic() >= deadline:
                raise ValueError('native upload HTTP service unavailable; deploy tools with --restart') from error
            time.sleep(0.1)
        finally:
            connection.close()

def receive(handler, backend=m):
    with backend.locked('gt710-upload'):
        intent=backend.read_json(backend.STATE/'gt710-upload.json',{})
        lease=backend.read_lease() or {}
        route=urlsplit(handler.path).path
        root_intent = intent
        sequence = (intent.get('suite') or '').split(',')
        suite_directory = None
        if len(sequence) > 1:
            scoped = re.fullmatch(re.escape(intent.get('route', '')) + r'/suites/(console|gpu|breengel|window-manager|ui-testing|gui-apps)(/.*)?', route)
            if not scoped or scoped[1] not in sequence:
                handler.send_error(404);return
            suite = scoped[1]
            intent = root_intent.setdefault('suiteSlots', {}).setdefault(suite, dict(
                id=root_intent['id'], route=root_intent['route'] + '/suites/' + suite, suite=suite))
            suite_directory = suite
        boot_directory=backend.boot_dir(intent['id']) if intent else None
        proof = bool(boot_directory) and backend.read_json(boot_directory / 'run.json', {}).get('screenProof') is True
        screen = re.fullmatch(re.escape(intent.get('route', '')) + r'/screens/pictures/(0|[1-9]\d*)', route) if intent else None
        screen_index = int(screen[1]) if screen else None
        is_screen = screen_index is not None
        final=bool(intent) and route==intent.get('route','')+'/final'
        picture = re.fullmatch(re.escape(intent.get('route', '')) + r'/pictures/(0|[1-9]\d*)', route) if intent else None
        picture_index = int(picture[1]) if picture else None
        is_picture = picture_index is not None
        diagnostic = re.fullmatch(re.escape(intent.get('route', '')) + r'/diagnostics/([a-z][a-z0-9-]{0,63})', route) if intent else None
        diagnostic_label = diagnostic[1] if diagnostic else None
        is_diagnostic = diagnostic_label is not None
        if (not intent or (route!=intent.get('route') and not final and not is_picture and not is_diagnostic and not is_screen)
                or (is_screen and (not proof or intent.get('suite') not in ('gpu', 'breengel') or not intent.get('complete')
                    or intent.get('finalComplete') or screen_index != intent.get('screenUploads', 0)))
                or (is_picture and (intent.get('suite') not in (('gpu', 'breengel', 'window-manager', 'ui-testing', 'gui-apps') if proof else ('breengel', 'window-manager', 'ui-testing', 'gui-apps')) or not intent.get('complete')
                    or intent.get('finalComplete') or picture_index != intent.get('pictureUploads', 0)))
                or (is_diagnostic and (not intent.get('complete') or intent.get('finalComplete') or
                    ((len(intent.get('diagnosticUploads', {})) >= 16 or
                      (intent.get('suite') == 'ui-testing' and not re.fullmatch(r'uitest-[0-9a-f]{16}', diagnostic_label)))
                     if intent.get('suite') in ('window-manager', 'ui-testing', 'gui-apps') else
                     not (proof and intent.get('suite') in ('gpu', 'breengel') and
                          re.fullmatch(r'crc-' + intent['suite'] + r'-[0-9a-f]{16}', diagnostic_label) and
                          len(intent.get('diagnosticUploads', {})) < crc_checkpoint_cap(intent['suite'])))))
                or (intent.get('finalComplete') if final else (False if is_picture or is_diagnostic or is_screen else intent.get('complete')))
                or (final and not intent.get('complete'))
                or lease.get('id')!=intent.get('id') or lease.get('state')!='powered'):
            handler.send_error(404);return
        boot_directory=backend.boot_dir(intent['id'])
        directory = boot_directory / 'native-suites' / suite_directory if suite_directory else boot_directory
        directory.mkdir(parents=True, exist_ok=True)
        filename = f'screen-part-{screen_index:04}.bin' if is_screen else f'diagnostic-{diagnostic_label}.bin' if is_diagnostic else f'gt710-breengel-{picture_index:04}.rle' if is_picture else ('gt710-final.rle' if final else 'gt710-pixels.rle')
        # Publishing the verified payload consumes the slot even if the
        # bookkeeping below has not finished (or its writer was interrupted).
        if (directory/filename).exists():
            handler.send_error(404);return
        temporary=directory/(filename+'.partial')
        try:
            length=int(handler.headers.get('Content-Length','-1'))
            expected=handler.headers.get('X-Content-SHA256','')
            if (handler.headers.get('Transfer-Encoding') or not 0<length<=MAX_BYTES
                    or not re.fullmatch('[a-f0-9]{64}',expected) or (boot_directory/'ended').exists()):
                raise ValueError('invalid display artifact framing')
            total = root_intent.get('aggregateBytes', 0) if suite_directory else intent.get('bytes', 0) + intent.get('pictureBytes', 0) + intent.get('diagnosticBytes', 0) + intent.get('screenBytes', 0)
            if total + length > MAX_TOTAL:
                raise ValueError('native per-boot evidence exceeds 64 MiB')
            handler.connection.settimeout(30)
            digest=hashlib.sha256();remaining=length
            with temporary.open('wb') as out:
                while remaining:
                    chunk=handler.rfile.read(min(65536,remaining))
                    if not chunk:raise ValueError('short display artifact')
                    out.write(chunk);digest.update(chunk);remaining-=len(chunk)
                out.flush();os.fsync(out.fileno())
            if digest.hexdigest()!=expected:raise ValueError('display artifact digest differs')
            payload_length, payload_digest = length, expected
            reference = handler.headers.get('X-Breenix-Picture-Reference')
            if reference is not None:
                if reference != '1' or not is_picture or not proof or length > 200:
                    raise ValueError('picture reference is not admitted')
                match = re.fullmatch(rb'PICTURE_REF v=1 source=(0|[1-9][0-9]*) bytes=([1-9][0-9]*) sha256=([0-9a-f]{64})\n', temporary.read_bytes())
                if not match:
                    raise ValueError('invalid picture reference')
                source_index, payload_length = int(match[1]), int(match[2])
                payload_digest = match[3].decode('ascii')
                source = directory / f'gt710-breengel-{source_index:04}.rle'
                source_receipt = backend.read_json(directory / f'native-picture-upload-{source_index:04}.json', {})
                if (source_index >= picture_index or not 0 < payload_length <= MAX_BYTES
                        or source.is_symlink() or not source.is_file()
                        or source_receipt.get('id') != intent['id'] or source_receipt.get('complete') is not True
                        or source_receipt.get('bytes') != payload_length or source_receipt.get('sha256') != payload_digest):
                    raise ValueError('picture reference source is not verified in this boot')
                body = source.read_bytes()
                if len(body) != payload_length or hashlib.sha256(body).hexdigest() != payload_digest:
                    raise ValueError('picture reference source digest differs')
                with temporary.open('wb') as out:
                    out.write(body);out.flush();os.fsync(out.fileno())
            if is_picture and intent.get('picturePayloadBytes', intent.get('pictureBytes', 0)) + payload_length > MAX_TOTAL:
                raise ValueError('native picture payload exceeds 64 MiB')
            temporary.replace(directory/filename)
            receipt=dict(id=intent['id'],route=route,complete=True,bytes=payload_length,
                         sha256=payload_digest,receivedAt=m.now())
            if reference is not None:
                receipt.update(wireBytes=length, wireSha256=expected, reference=source_index)
            if is_screen:
                intent['screenUploads'] = screen_index + 1
                intent['screenBytes'] = intent.get('screenBytes', 0) + length
            elif is_diagnostic:
                intent.setdefault('diagnosticUploads', {})[diagnostic_label] = receipt
                intent['diagnosticBytes'] = intent.get('diagnosticBytes', 0) + length
            elif is_picture:
                intent['pictureUploads'] = picture_index + 1
                intent['picturePayloadBytes'] = intent.get('picturePayloadBytes', intent.get('pictureBytes', 0)) + payload_length
                intent['pictureBytes'] = intent.get('pictureBytes', 0) + length
            elif final:
                intent['finalComplete']=True
                intent['finalBytes']=length
            else:
                intent.update(receipt)
            handler.send_response(200)
            if reference is not None:
                handler.send_header('X-Breenix-Picture-Reference-Accepted', '1')
            handler.send_header('Content-Length','0')
            handler.send_header('Connection','close');handler.end_headers();handler.close_connection=True
        except (OSError,ValueError) as error:
            temporary.unlink(missing_ok=True);handler.close_connection=True
            backend.write_json(directory/'native-upload-refused.json', dict(route=route, reason=str(error), at=m.now()))
            handler.send_error(400,'display artifact upload failed')
            return
        # The payload is synced and digest-checked before HTTP acceptance.
        # Receipt/state fsyncs are host bookkeeping, outside the guest's reply
        # window. Keep the slot lock until they finish; a persistence error
        # propagates to the service instead of attempting a second HTTP status
        # after the 200 already sent.
        receipt_name = f'screen-part-upload-{screen_index:04}.json' if is_screen else f'native-diagnostic-{diagnostic_label}.json' if is_diagnostic else f'native-picture-upload-{picture_index:04}.json' if is_picture else ('native-final-upload.json' if final else 'native-upload.json')
        backend.write_json(directory/receipt_name,receipt)
        if suite_directory:
            root_intent['aggregateBytes'] = total + length
        backend.write_json(backend.STATE/'gt710-upload.json',root_intent)
