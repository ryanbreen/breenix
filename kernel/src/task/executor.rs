use super::{Task, TaskId};
use alloc::{collections::BTreeMap, sync::Arc, task::Wake};
use core::task::{Context, Poll, Waker};
use crossbeam_queue::ArrayQueue;
#[cfg(target_arch = "x86_64")]
use x86_64::instructions::interrupts;

#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
pub struct Executor {
    tasks: BTreeMap<TaskId, Task>,
    task_queue: Arc<ArrayQueue<TaskId>>,
    waker_cache: BTreeMap<TaskId, Waker>,
}

impl Executor {
    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    pub fn new() -> Self {
        Executor {
            tasks: BTreeMap::new(),
            task_queue: Arc::new(ArrayQueue::new(100)),
            waker_cache: BTreeMap::new(),
        }
    }

    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    pub fn spawn(&mut self, task: Task) {
        let task_id = task.id;
        if self.tasks.insert(task.id, task).is_some() {
            panic!("task with same ID already in tasks");
        }
        self.task_queue.push(task_id).expect("queue full");
    }

    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    fn run_ready_tasks(&mut self) {
        // destructure `self` to avoid borrow checker errors
        let Self {
            tasks,
            task_queue,
            waker_cache,
        } = self;

        while let Some(task_id) = task_queue.pop() {
            let task = match tasks.get_mut(&task_id) {
                Some(task) => task,
                None => continue, // task no longer exists
            };
            let waker = waker_cache
                .entry(task_id)
                .or_insert_with(|| TaskWaker::new(task_id, task_queue.clone()));
            let mut context = Context::from_waker(waker);
            match task.poll(&mut context) {
                Poll::Ready(()) => {
                    // task done -> remove it and its cached waker
                    tasks.remove(&task_id);
                    waker_cache.remove(&task_id);
                }
                Poll::Pending => {}
            }
        }
    }

    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    #[cfg(target_arch = "x86_64")]
    fn sleep_if_idle(&self) {
        interrupts::disable();
        if self.task_queue.is_empty() {
            interrupts::enable();
            x86_64::instructions::hlt();
        } else {
            interrupts::enable();
        }
    }

    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    #[cfg(not(target_arch = "x86_64"))]
    fn sleep_if_idle(&self) {
        if self.task_queue.is_empty() {
            core::hint::spin_loop();
        }
    }

    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    pub fn run(&mut self) -> ! {
        loop {
            self.run_ready_tasks();
            self.sleep_if_idle();
        }
    }
}

#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
struct TaskWaker {
    task_id: TaskId,
    task_queue: Arc<ArrayQueue<TaskId>>,
}

impl TaskWaker {
    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    fn new(task_id: TaskId, task_queue: Arc<ArrayQueue<TaskId>>) -> Waker {
        Waker::from(Arc::new(TaskWaker {
            task_id,
            task_queue,
        }))
    }

    #[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
    fn wake_task(&self) {
        self.task_queue.push(self.task_id).expect("task_queue full");
    }
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_task();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_task();
    }
}
