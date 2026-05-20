# Turn 34: Linux VirtIO GPU control queue profile

## Probe and source provenance

Live probe:

- Host: `10.211.55.3`
- Kernel: `6.8.0-111-generic`
- Architecture: `aarch64`
- Module: `virtio_gpu`
- Module file:
  `/lib/modules/6.8.0-111-generic/kernel/drivers/gpu/drm/virtio/virtio-gpu.ko.zst`

The probe has kernel headers under `/usr/src/linux-headers-6.8.0-111-generic`,
but those headers do not include `drivers/gpu/drm/virtio/`. For source citations
this artifact uses captured upstream stable v6.8 source excerpts from
kernel.org under `drivers/gpu/drm/virtio/`.

The live ftrace on the probe confirms that the active Ubuntu driver uses the same
symbol chain profiled below: `virtio_gpu_notify`, `virtio_gpu_ctrl_ack`,
`virtio_gpu_dequeue_ctrl_func`, and `virtqueue_get_buf()`.

## Queue state in Linux

`drivers/gpu/drm/virtio/virtgpu_drv.h:197-203` defines each GPU queue as a
virtqueue plus a spinlock, waitqueue, work item, and sequence number:

```c
struct virtio_gpu_queue {
	struct virtqueue *vq;
	spinlock_t qlock;
	wait_queue_head_t ack_queue;
	struct work_struct dequeue_work;
	uint32_t seqno;
};
```

`drivers/gpu/drm/virtio/virtgpu_drv.h:228-232` stores both `ctrlq` and `cursorq`
on the device and tracks pending control commands:

```c
	struct virtio_gpu_queue ctrlq;
	struct virtio_gpu_queue cursorq;
	struct kmem_cache *vbufs;

	atomic_t pending_commands;
```

## Key function 1: IRQ virtqueue callback

`drivers/gpu/drm/virtio/virtgpu_vq.c:56-75`:

```c
void virtio_gpu_ctrl_ack(struct virtqueue *vq)
{
	struct drm_device *dev = vq->vdev->priv;
	struct virtio_gpu_device *vgdev = dev->dev_private;

	schedule_work(&vgdev->ctrlq.dequeue_work);
}

void virtio_gpu_cursor_ack(struct virtqueue *vq)
{
	struct drm_device *dev = vq->vdev->priv;
	struct virtio_gpu_device *vgdev = dev->dev_private;

	schedule_work(&vgdev->cursorq.dequeue_work);
}

int virtio_gpu_alloc_vbufs(struct virtio_gpu_device *vgdev)
{
	vgdev->vbufs = kmem_cache_create("virtio-gpu-vbufs",
```

Linux's virtqueue callback does not drain the queue in hard IRQ context. It
schedules work for the control queue.

## Key function 2: deferred control queue drain

`drivers/gpu/drm/virtio/virtgpu_vq.c:196-245`:

```c
void virtio_gpu_dequeue_ctrl_func(struct work_struct *work)
{
	struct virtio_gpu_device *vgdev =
		container_of(work, struct virtio_gpu_device,
			     ctrlq.dequeue_work);
	struct list_head reclaim_list;
	struct virtio_gpu_vbuffer *entry, *tmp;
	struct virtio_gpu_ctrl_hdr *resp;
	u64 fence_id;

	INIT_LIST_HEAD(&reclaim_list);
	spin_lock(&vgdev->ctrlq.qlock);
	do {
		virtqueue_disable_cb(vgdev->ctrlq.vq);
		reclaim_vbufs(vgdev->ctrlq.vq, &reclaim_list);

	} while (!virtqueue_enable_cb(vgdev->ctrlq.vq));
	spin_unlock(&vgdev->ctrlq.qlock);

	list_for_each_entry(entry, &reclaim_list, list) {
		resp = (struct virtio_gpu_ctrl_hdr *)entry->resp_buf;

		trace_virtio_gpu_cmd_response(vgdev->ctrlq.vq, resp, entry->seqno);

		if (resp->type != cpu_to_le32(VIRTIO_GPU_RESP_OK_NODATA)) {
			if (le32_to_cpu(resp->type) >= VIRTIO_GPU_RESP_ERR_UNSPEC) {
				struct virtio_gpu_ctrl_hdr *cmd;
				cmd = virtio_gpu_vbuf_ctrl_hdr(entry);
				DRM_ERROR_RATELIMITED("response 0x%x (command 0x%x)\n",
						      le32_to_cpu(resp->type),
						      le32_to_cpu(cmd->type));
			} else
				DRM_DEBUG("response 0x%x\n", le32_to_cpu(resp->type));
		}
		if (resp->flags & cpu_to_le32(VIRTIO_GPU_FLAG_FENCE)) {
			fence_id = le64_to_cpu(resp->fence_id);
			virtio_gpu_fence_event_process(vgdev, fence_id);
		}
		if (entry->resp_cb)
			entry->resp_cb(vgdev, entry);
	}
	wake_up(&vgdev->ctrlq.ack_queue);

	list_for_each_entry_safe(entry, tmp, &reclaim_list, list) {
		if (entry->objs)
			virtio_gpu_array_put_free_delayed(vgdev, entry->objs);
		list_del(&entry->list);
		free_vbuf(vgdev, entry);
	}
}
```

The work item disables callbacks, drains all completed used-ring entries with
`reclaim_vbufs()`, then re-enables callbacks in a loop to close the usual
disable/drain/enable race. It processes fence completions and response callbacks,
wakes `ctrlq.ack_queue`, and frees vbuffers.

The helper `reclaim_vbufs()` is at
`drivers/gpu/drm/virtio/virtgpu_vq.c:182-194` and repeatedly calls
`virtqueue_get_buf(vq, &len)`.

## Key function 3: submission path

The public wrapper is small.
`drivers/gpu/drm/virtio/virtgpu_vq.c:425-445`:

```c
void virtio_gpu_notify(struct virtio_gpu_device *vgdev)
{
	bool notify;

	if (!atomic_read(&vgdev->pending_commands))
		return;

	spin_lock(&vgdev->ctrlq.qlock);
	atomic_set(&vgdev->pending_commands, 0);
	notify = virtqueue_kick_prepare(vgdev->ctrlq.vq);
	spin_unlock(&vgdev->ctrlq.qlock);

	if (notify)
		virtqueue_notify(vgdev->ctrlq.vq);
}

static int virtio_gpu_queue_ctrl_buffer(struct virtio_gpu_device *vgdev,
					struct virtio_gpu_vbuffer *vbuf)
{
	return virtio_gpu_queue_fenced_ctrl_buffer(vgdev, vbuf, NULL);
}
```

The core enqueue path is `drivers/gpu/drm/virtio/virtgpu_vq.c:314-369`:

```c
static int virtio_gpu_queue_ctrl_sgs(struct virtio_gpu_device *vgdev,
				     struct virtio_gpu_vbuffer *vbuf,
				     struct virtio_gpu_fence *fence,
				     int elemcnt,
				     struct scatterlist **sgs,
				     int outcnt,
				     int incnt)
{
	struct virtqueue *vq = vgdev->ctrlq.vq;
	int ret, idx;

	if (!drm_dev_enter(vgdev->ddev, &idx)) {
		if (fence && vbuf->objs)
			virtio_gpu_array_unlock_resv(vbuf->objs);
		free_vbuf(vgdev, vbuf);
		return -ENODEV;
	}

	if (vgdev->has_indirect)
		elemcnt = 1;

again:
	spin_lock(&vgdev->ctrlq.qlock);

	if (vq->num_free < elemcnt) {
		spin_unlock(&vgdev->ctrlq.qlock);
		virtio_gpu_notify(vgdev);
		wait_event(vgdev->ctrlq.ack_queue, vq->num_free >= elemcnt);
		goto again;
	}

	/* now that the position of the vbuf in the virtqueue is known, we can
	 * finally set the fence id
	 */
	if (fence) {
		virtio_gpu_fence_emit(vgdev, virtio_gpu_vbuf_ctrl_hdr(vbuf),
				      fence);
		if (vbuf->objs) {
			virtio_gpu_array_add_fence(vbuf->objs, &fence->f);
			virtio_gpu_array_unlock_resv(vbuf->objs);
		}
	}

	ret = virtqueue_add_sgs(vq, sgs, outcnt, incnt, vbuf, GFP_ATOMIC);
	WARN_ON(ret);

	vbuf->seqno = ++vgdev->ctrlq.seqno;
	trace_virtio_gpu_cmd_queue(vq, virtio_gpu_vbuf_ctrl_hdr(vbuf), vbuf->seqno);

	atomic_inc(&vgdev->pending_commands);

	spin_unlock(&vgdev->ctrlq.qlock);

	drm_dev_exit(idx);
	return 0;
}
```

Linux separates enqueue from kick. Commands increment `pending_commands`; a later
`virtio_gpu_notify()` does `virtqueue_kick_prepare()` and `virtqueue_notify()`.

## Key function 4: waiter surface

For userspace object waits,
`drivers/gpu/drm/virtio/virtgpu_ioctl.c:341-365` uses reservation-object waiting
rather than spinning:

```c
static int virtio_gpu_wait_ioctl(struct drm_device *dev, void *data,
				 struct drm_file *file)
{
	struct drm_virtgpu_3d_wait *args = data;
	struct drm_gem_object *obj;
	long timeout = 15 * HZ;
	int ret;

	obj = drm_gem_object_lookup(file, args->handle);
	if (obj == NULL)
		return -ENOENT;

	if (args->flags & VIRTGPU_WAIT_NOWAIT) {
		ret = dma_resv_test_signaled(obj->resv, DMA_RESV_USAGE_READ);
	} else {
		ret = dma_resv_wait_timeout(obj->resv, DMA_RESV_USAGE_READ,
					    true, timeout);
	}
	if (ret == 0)
		ret = -EBUSY;
	else if (ret > 0)
		ret = 0;

	drm_gem_object_put(obj);
	return ret;
```

The kernel command completion path flows through the dequeue work above. Fenced
responses call `virtio_gpu_fence_event_process()` in `virtgpu_vq.c:230-233`, and
the dequeue work wakes `ctrlq.ack_queue` in `virtgpu_vq.c:237`.

## Live ftrace evidence

Raw trace: `turn34-artifacts/linux-virtgpu-ftrace.txt`.

Filter setup note: `virtio_gpu_queue_ctrl_buffer` was not available to ftrace
as a filterable function on the probe, likely because it is static/inlined in the
Ubuntu build. The captured filter did include `virtio_gpu_notify`,
`virtio_gpu_ctrl_ack`, `virtio_gpu_dequeue_ctrl_func`, `virtio_gpu_wait_ioctl`,
`virtqueue_notify`, `virtqueue_kick_prepare`, `virtqueue_get_buf*`,
`vring_interrupt`, and IRQ entry/exit events.

Trigger: four small writes to `/dev/fb0`.

Key trace excerpt:

```text
kworker/1:1-6214 [001] ..... 83409.147470: virtio_gpu_notify <-virtio_gpu_primary_plane_update
kworker/1:1-6214 [001] ...1. 83409.147470: virtqueue_kick_prepare <-virtio_gpu_notify
kworker/1:1-6214 [001] ..... 83409.147470: virtqueue_notify <-virtio_gpu_notify
<idle>-0       [000] d.h1. 83409.148066: irq_handler_entry: irq=28 name=virtio2-virtqueues
<idle>-0       [000] d.h1. 83409.148067: vp_vring_interrupt <-__handle_irq_event_percpu
<idle>-0       [000] d.h2. 83409.148067: vring_interrupt <-vp_vring_interrupt
<idle>-0       [000] d.h2. 83409.148068: virtio_gpu_ctrl_ack <-vring_interrupt
<idle>-0       [000] dNh1. 83409.148072: irq_handler_exit: irq=28 ret=handled
kworker/0:0-5760 [000] ..... 83409.148079: virtio_gpu_dequeue_ctrl_func <-process_one_work
kworker/0:0-5760 [000] ...1. 83409.148080: virtqueue_get_buf <-reclaim_vbufs
kworker/0:0-5760 [000] ...1. 83409.148080: virtqueue_get_buf_ctx_split <-virtqueue_get_buf
```

This gives the observed chain:

```text
fbdev update
  -> virtio_gpu_notify()
  -> virtqueue_kick_prepare()
  -> virtqueue_notify()
  -> device raises virtio2-virtqueues IRQ
  -> vp_vring_interrupt()
  -> vring_interrupt()
  -> virtio_gpu_ctrl_ack()
  -> schedule_work(ctrlq.dequeue_work)
  -> virtio_gpu_dequeue_ctrl_func()
  -> reclaim_vbufs()
  -> virtqueue_get_buf()
  -> process response callbacks/fences
  -> wake_up(ctrlq.ack_queue)
```

## Linux profile summary

Linux does not poll `used.idx` after a VirtIO GPU control queue kick. Submission
adds one or more scatter-gather descriptors to `ctrlq.vq`, increments
`pending_commands`, and later kicks through `virtio_gpu_notify()`. Completion is
delivered by the virtqueue IRQ callback `virtio_gpu_ctrl_ack()`, which only
schedules work. The work item drains used-ring entries, processes responses and
fences, wakes queue waiters, and frees vbuffers.
