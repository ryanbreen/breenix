"""Private, bounded actual-display-row upload for the powered boot's lease."""
import hashlib
import os
from pathlib import Path
import re
import secrets
from urllib.parse import urlsplit, urlunsplit
import metal_lib as m
MAX_BYTES=2<<20
PREFIX='/gt710-upload/'
def prepare(directory,boot_id,site):
    route=PREFIX+secrets.token_hex(32)
    intent=dict(id=boot_id,route=route)
    m.write_json(m.STATE/'gt710-upload.json',intent)
    m.write_json(Path(directory)/'native-upload-intent.json',intent)
    url=urlsplit(site['BOOT_URL'])
    return urlunsplit((url.scheme,url.netloc,route,'',''))
def receive(handler):
    with m.locked('gt710-upload'):
        intent=m.read_json(m.STATE/'gt710-upload.json',{})
        lease=m.read_lease() or {}
        route=urlsplit(handler.path).path
        final=bool(intent) and route==intent.get('route','')+'/final'
        if (not intent or (route!=intent.get('route') and not final)
                or (intent.get('finalComplete') if final else intent.get('complete'))
                or (final and not intent.get('complete'))
                or lease.get('id')!=intent.get('id') or lease.get('state')!='powered'):
            handler.send_error(404);return
        directory=m.boot_dir(intent['id'])
        filename='gt710-final.rle' if final else 'gt710-pixels.rle'
        # Publishing the verified payload consumes the slot even if the
        # bookkeeping below has not finished (or its writer was interrupted).
        if (directory/filename).exists():
            handler.send_error(404);return
        temporary=directory/(filename+'.partial')
        try:
            length=int(handler.headers.get('Content-Length','-1'))
            expected=handler.headers.get('X-Content-SHA256','')
            if (handler.headers.get('Transfer-Encoding') or not 0<length<=MAX_BYTES
                    or not re.fullmatch('[a-f0-9]{64}',expected) or (directory/'ended').exists()):
                raise ValueError('invalid display artifact framing')
            handler.connection.settimeout(30)
            digest=hashlib.sha256();remaining=length
            with temporary.open('wb') as out:
                while remaining:
                    chunk=handler.rfile.read(min(65536,remaining))
                    if not chunk:raise ValueError('short display artifact')
                    out.write(chunk);digest.update(chunk);remaining-=len(chunk)
                out.flush();os.fsync(out.fileno())
            if digest.hexdigest()!=expected:raise ValueError('display artifact digest differs')
            temporary.replace(directory/filename)
            receipt=dict(id=intent['id'],route=route,complete=True,bytes=length,
                         sha256=expected,receivedAt=m.now())
            if final:
                intent['finalComplete']=True
            else:
                intent.update(receipt)
            handler.send_response(200);handler.send_header('Content-Length','0')
            handler.send_header('Connection','close');handler.end_headers();handler.close_connection=True
        except (OSError,ValueError):
            temporary.unlink(missing_ok=True);handler.close_connection=True
            handler.send_error(400,'display artifact upload failed')
            return
        # The payload is synced and digest-checked before HTTP acceptance.
        # Receipt/state fsyncs are host bookkeeping, outside the guest's reply
        # window. Keep the slot lock until they finish; a persistence error
        # propagates to the service instead of attempting a second HTTP status
        # after the 200 already sent.
        m.write_json(directory/('native-final-upload.json' if final else 'native-upload.json'),receipt)
        m.write_json(m.STATE/'gt710-upload.json',intent)
