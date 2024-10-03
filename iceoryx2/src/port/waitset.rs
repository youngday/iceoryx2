// Copyright (c) 2024 Contributors to the Eclipse Foundation
//
// See the NOTICE file(s) distributed with this work for additional
// information regarding copyright ownership.
//
// This program and the accompanying materials are made available under the
// terms of the Apache Software License 2.0 which is available at
// https://www.apache.org/licenses/LICENSE-2.0, or the MIT license
// which is available at https://opensource.org/licenses/MIT.
//
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! # Example
//!
//! ## Use [`AttachmentId::originates_from()`](crate::port::waitset::AttachmentId)
//!
//! ```no_run
//! use iceoryx2::prelude::*;
//! # use core::time::Duration;
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let node = NodeBuilder::new().create::<ipc::Service>()?;
//! # let event_1 = node.service_builder(&"MyEventName_1".try_into()?)
//! #     .event()
//! #     .open_or_create()?;
//! # let event_2 = node.service_builder(&"MyEventName_2".try_into()?)
//! #     .event()
//! #     .open_or_create()?;
//!
//! let mut listener_1 = event_1.listener_builder().create()?;
//! let mut listener_2 = event_2.listener_builder().create()?;
//!
//! let waitset = WaitSetBuilder::new().create::<ipc::Service>()?;
//! let _guard_1 = waitset.attach(&listener_1)?;
//! let _guard_2 = waitset.attach(&listener_2)?;
//!
//! let event_handler = |attachment_id| {
//!     let listener = if attachment_id.originates_from(&listener_1) {
//!         &listener_1
//!     } else {
//!         &listener_2
//!     };
//!
//!     while let Ok(Some(event_id)) = listener.try_wait_one() {
//!         println!("received notification {:?}", event_id);
//!     }
//! };
//!
//! while waitset.timed_wait(event_handler, Duration::from_secs(1))
//!     != Ok(WaitEvent::TerminationRequest) {}
//!
//! # Ok(())
//! # }
//! ```
//!
//! ## [`HashMap`](std::collections::HashMap) approach
//!
//! ```no_run
//! use iceoryx2::prelude::*;
//! use std::collections::HashMap;
//! use iceoryx2::port::listener::Listener;
//! # use core::time::Duration;
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let node = NodeBuilder::new().create::<ipc::Service>()?;
//! # let event_1 = node.service_builder(&"MyEventName_1".try_into()?)
//! #     .event()
//! #     .open_or_create()?;
//! # let event_2 = node.service_builder(&"MyEventName_2".try_into()?)
//! #     .event()
//! #     .open_or_create()?;
//!
//! let mut listeners: HashMap<AttachmentId, Listener<ipc::Service>> = HashMap::new();
//! let listener = event_1.listener_builder().create()?;
//! listeners.insert(AttachmentId::new(&listener), listener);
//!
//! let listener = event_2.listener_builder().create()?;
//! listeners.insert(AttachmentId::new(&listener), listener);
//!
//! let waitset = WaitSetBuilder::new().create::<ipc::Service>()?;
//! let mut guards = vec![];
//! for listener in listeners.values() {
//!     guards.push(waitset.attach(listener)?);
//! }
//!
//! while waitset.timed_wait(|attachment_id| {
//!     if let Some(listener) = listeners.get(&attachment_id) {
//!         while let Ok(Some(event_id)) = listener.try_wait_one() {
//!             println!("received notification {:?}", event_id);
//!         }
//!     }
//! }, Duration::from_secs(1)) != Ok(WaitEvent::TerminationRequest) {}
//!
//! # Ok(())
//! # }
//! ```
//!

use std::{
    cell::RefCell, collections::HashMap, fmt::Debug, hash::Hash, marker::PhantomData,
    time::Duration,
};

use iceoryx2_bb_log::fail;
use iceoryx2_bb_posix::{
    file_descriptor_set::SynchronousMultiplexing,
    signal::SignalHandler,
    timer::{Timer, TimerBuilder, TimerGuard, TimerIndex},
};
use iceoryx2_cal::reactor::*;

/// Defines the type of that triggered [`WaitSet::try_wait()`], [`WaitSet::timed_wait()`] or
/// [`WaitSet::blocking_wait()`].
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum WaitEvent {
    /// A termination signal `SIGTERM` was received.
    TerminationRequest,
    /// An interrupt signal `SIGINT` was received.
    Interrupt,
    /// No event was triggered.
    Tick,
    /// One or more event notifications were received.
    Notification,
}

/// Defines the failures that can occur when attaching something with [`WaitSet::attach()`].
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum WaitSetAttachmentError {
    /// The [`WaitSet`]s capacity is exceeded.
    InsufficientCapacity,
    /// The attachment is already attached.
    AlreadyAttached,
    /// An internal error has occurred.
    InternalError,
}

impl std::fmt::Display for WaitSetAttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::write!(f, "WaitSetAttachmentError::{:?}", self)
    }
}

impl std::error::Error for WaitSetAttachmentError {}

/// Defines the failures that can occur when calling
///  * [`WaitSet::try_wait()`]
///  * [`WaitSet::timed_wait()`]
///  * [`WaitSet::blocking_wait()`]
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum WaitSetWaitError {
    /// The process has not sufficient permissions to wait on the attachments.
    InsufficientPermissions,
    /// An internal error has occurred.
    InternalError,
}

impl std::fmt::Display for WaitSetWaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::write!(f, "WaitSetWaitError::{:?}", self)
    }
}

impl std::error::Error for WaitSetWaitError {}

/// Defines the failures that can occur when calling [`WaitSetBuilder::create()`].
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum WaitSetCreateError {
    /// An internal error has occurred.
    InternalError,
}

impl std::fmt::Display for WaitSetCreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::write!(f, "WaitSetCreateError::{:?}", self)
    }
}

impl std::error::Error for WaitSetCreateError {}

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
enum AttachmentIdType {
    Tick(u64, TimerIndex),
    Deadline(u64, i32, TimerIndex),
    Notification(u64, i32),
}

/// Represents an attachment to the [`WaitSet`]
#[derive(Debug, Clone, Copy)]
pub struct AttachmentId<Service: crate::service::Service> {
    attachment_type: AttachmentIdType,
    _data: PhantomData<Service>,
}

impl<Service: crate::service::Service> PartialEq for AttachmentId<Service> {
    fn eq(&self, other: &Self) -> bool {
        self.attachment_type == other.attachment_type
    }
}

impl<Service: crate::service::Service> Eq for AttachmentId<Service> {}

impl<Service: crate::service::Service> Hash for AttachmentId<Service> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.attachment_type.hash(state)
    }
}

impl<Service: crate::service::Service> AttachmentId<Service> {
    fn tick(waitset: &WaitSet<Service>, timer_idx: TimerIndex) -> Self {
        Self {
            attachment_type: AttachmentIdType::Tick(
                waitset as *const WaitSet<Service> as u64,
                timer_idx,
            ),
            _data: PhantomData,
        }
    }

    fn deadline(waitset: &WaitSet<Service>, reactor_idx: i32, timer_idx: TimerIndex) -> Self {
        Self {
            attachment_type: AttachmentIdType::Deadline(
                waitset as *const WaitSet<Service> as u64,
                reactor_idx,
                timer_idx,
            ),
            _data: PhantomData,
        }
    }

    fn notification(waitset: &WaitSet<Service>, reactor_idx: i32) -> Self {
        Self {
            attachment_type: AttachmentIdType::Notification(
                waitset as *const WaitSet<Service> as u64,
                reactor_idx,
            ),
            _data: PhantomData,
        }
    }

    /// Returns true if the attachment originated from `other`
    pub fn originates_from(&self, other: &Guard<Service>) -> bool {
        self.attachment_type == other.to_attachment_id().attachment_type
    }
}

enum GuardType<'waitset, 'attachment, Service: crate::service::Service>
where
    Service::Reactor: 'waitset,
{
    Tick(TimerGuard<'waitset>),
    Deadline(
        <Service::Reactor as Reactor>::Guard<'waitset, 'attachment>,
        TimerGuard<'waitset>,
    ),
    Notification(<Service::Reactor as Reactor>::Guard<'waitset, 'attachment>),
}

/// Is returned when something is attached to the [`WaitSet`]. As soon as it goes out
/// of scope, the attachment is detached.
pub struct Guard<'waitset, 'attachment, Service: crate::service::Service>
where
    Service::Reactor: 'waitset,
{
    waitset: &'waitset WaitSet<Service>,
    guard_type: GuardType<'waitset, 'attachment, Service>,
}

impl<'waitset, 'attachment, Service: crate::service::Service>
    Guard<'waitset, 'attachment, Service>
{
    /// Extracts the [`AttachmentId`] from the guard.
    pub fn to_attachment_id(&self) -> AttachmentId<Service> {
        match &self.guard_type {
            GuardType::Tick(t) => AttachmentId::tick(self.waitset, t.index()),
            GuardType::Deadline(r, t) => AttachmentId::deadline(
                self.waitset,
                unsafe { r.file_descriptor().native_handle() },
                t.index(),
            ),
            GuardType::Notification(r) => AttachmentId::notification(self.waitset, unsafe {
                r.file_descriptor().native_handle()
            }),
        }
    }
}

impl<'waitset, 'attachment, Service: crate::service::Service> Drop
    for Guard<'waitset, 'attachment, Service>
{
    fn drop(&mut self) {
        if let GuardType::Deadline(r, t) = &self.guard_type {
            self.waitset
                .remove_deadline(unsafe { r.file_descriptor().native_handle() }, t.index())
        }
    }
}

/// The builder for the [`WaitSet`].
#[derive(Debug)]
pub struct WaitSetBuilder {}

impl Default for WaitSetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitSetBuilder {
    /// Creates a new [`WaitSetBuilder`].
    pub fn new() -> Self {
        Self {}
    }

    /// Creates the [`WaitSet`].
    pub fn create<'waitset, Service: crate::service::Service>(
        self,
    ) -> Result<WaitSet<Service>, WaitSetCreateError> {
        let msg = "Unable to create WaitSet";
        let timer = fail!(from self, when TimerBuilder::new().create(),
                with WaitSetCreateError::InternalError,
                "{msg} since the underlying Timer could not be created.");

        match <Service::Reactor as Reactor>::Builder::new().create() {
            Ok(reactor) => Ok(WaitSet {
                reactor,
                timer,
                deadline_to_timer: RefCell::new(HashMap::new()),
                timer_to_deadline: RefCell::new(HashMap::new()),
            }),
            Err(ReactorCreateError::UnknownError(e)) => {
                fail!(from self, with WaitSetCreateError::InternalError,
                    "{msg} due to an internal error (error code = {})", e);
            }
        }
    }
}

/// The [`WaitSet`] implements a reactor pattern and allows to wait on multiple events in one
/// single call [`WaitSet::try_wait()`], [`WaitSet::timed_wait()`] or [`WaitSet::blocking_wait()`].
///
/// An struct must implement [`SynchronousMultiplexing`] to be attachable. The
/// [`Listener`](crate::port::listener::Listener) can be attached as well as sockets or anything else that
/// is [`FileDescriptorBased`](iceoryx2_bb_posix::file_descriptor::FileDescriptorBased).
///
/// Can be created via the [`WaitSetBuilder`].
#[derive(Debug)]
pub struct WaitSet<Service: crate::service::Service> {
    reactor: Service::Reactor,
    timer: Timer,
    deadline_to_timer: RefCell<HashMap<i32, TimerIndex>>,
    timer_to_deadline: RefCell<HashMap<TimerIndex, i32>>,
}

impl<Service: crate::service::Service> WaitSet<Service> {
    fn remove_deadline<'waitset>(&'waitset self, reactor_idx: i32, timer_idx: TimerIndex) {
        self.deadline_to_timer.borrow_mut().remove(&reactor_idx);
        self.timer_to_deadline.borrow_mut().remove(&timer_idx);
    }

    fn contains_deadlines(&self) -> bool {
        !self.deadline_to_timer.borrow().is_empty()
    }

    fn reset_deadline(&self, reactor_idx: i32) -> Result<Option<TimerIndex>, WaitSetWaitError> {
        let msg = "Unable to reset deadline";
        if let Some(timer_idx) = self.deadline_to_timer.borrow().get(&reactor_idx) {
            fail!(from self,
                  when self.timer.reset(*timer_idx),
                  with WaitSetWaitError::InternalError,
                  "{msg} since the timer guard could not be reset for the attachment {reactor_idx}. Continuing operations will lead to invalid deadline failures.");
            Ok(Some(*timer_idx))
        } else {
            Ok(None)
        }
    }

    /// Attaches an object as notification to the [`WaitSet`]. Whenever an event is received on the
    /// object the [`WaitSet`] informs the user in [`WaitSet::run()`] to handle the event.
    /// The object cannot be attached twice and the
    /// [`WaitSet::capacity()`] is limited by the underlying implementation.
    pub fn notification<'waitset, 'attachment, T: SynchronousMultiplexing + Debug>(
        &'waitset self,
        attachment: &'attachment T,
    ) -> Result<Guard<'waitset, 'attachment, Service>, WaitSetAttachmentError> {
        Ok(Guard {
            waitset: self,
            guard_type: GuardType::Notification(self.attach_to_reactor(attachment)?),
        })
    }

    /// Attaches an object as deadline to the [`WaitSet`]. Whenever the event is received or the
    /// deadline is hit, the user is informed in [`WaitSet::run()`].
    /// The object cannot be attached twice and the
    /// [`WaitSet::capacity()`] is limited by the underlying implementation.
    /// Whenever the object emits an event the deadline is reset by the [`WaitSet`].
    pub fn deadline<'waitset, 'attachment, T: SynchronousMultiplexing + Debug>(
        &'waitset self,
        attachment: &'attachment T,
        deadline: Duration,
    ) -> Result<Guard<'waitset, 'attachment, Service>, WaitSetAttachmentError> {
        let reactor_guard = self.attach_to_reactor(attachment)?;
        let timer_guard = self.attach_to_timer(deadline)?;

        self.deadline_to_timer.borrow_mut().insert(
            unsafe { reactor_guard.file_descriptor().native_handle() },
            timer_guard.index(),
        );

        Ok(Guard {
            waitset: self,
            guard_type: GuardType::Deadline(reactor_guard, timer_guard),
        })
    }

    /// Attaches a tick event to the [`WaitSet`]. Whenever the timeout is reached the [`WaitSet`]
    /// informs the user in [`WaitSet::run()`].
    pub fn tick<'waitset>(
        &'waitset self,
        timeout: Duration,
    ) -> Result<Guard<'waitset, '_, Service>, WaitSetAttachmentError> {
        Ok(Guard {
            waitset: self,
            guard_type: GuardType::Tick(self.attach_to_timer(timeout)?),
        })
    }

    /// Tries to wait on the [`WaitSet`]. The provided callback is called for every attachment that
    /// was triggered and the [`AttachmentId`] is provided as an input argument to acquire the
    /// source.
    /// If nothing was triggered the [`WaitSet`] returns immediately.
    pub fn run<F: FnMut(AttachmentId<Service>)>(
        &self,
        mut fn_call: F,
    ) -> Result<WaitEvent, WaitSetWaitError> {
        if SignalHandler::termination_requested() {
            return Ok(WaitEvent::TerminationRequest);
        }

        let msg = "Unable to call WaitSet::run()";
        let next_timeout = fail!(from self,
                                 when self.timer.duration_until_next_timeout(),
                                 with WaitSetWaitError::InternalError,
                                 "{msg} since the next timeout could not be acquired.");

        let mut fds = vec![];
        match self.reactor.timed_wait(
            // Collect all triggered file descriptors. We need to collect them first, then reset
            // the deadline and then call the callback, otherwise a long callback may destroy the
            // deadline contract.
            |fd| {
                let fd = unsafe { fd.native_handle() };
                fds.push(fd);
            },
            next_timeout,
        ) {
            Ok(0) => {
                if self.contains_deadlines() {}
                self.timer
                    .missed_timeouts(|timer_idx| fn_call(AttachmentId::tick(self, timer_idx)))
                    .unwrap();

                Ok(WaitEvent::Tick)
            }
            Ok(n) => {
                // we need to reset the deadlines first, otherwise a long fn_call may extend the
                // deadline unintentionally
                if self.contains_deadlines() {
                    let mut fd_and_timer_idx = Vec::new();
                    fd_and_timer_idx.reserve(n);

                    for fd in &fds {
                        fd_and_timer_idx.push((fd, self.reset_deadline(*fd)?));
                    }

                    for (fd, timer_idx) in fd_and_timer_idx {
                        if let Some(timer_idx) = timer_idx {
                            fn_call(AttachmentId::deadline(self, *fd, timer_idx));
                        } else {
                            fn_call(AttachmentId::notification(self, *fd));
                        }
                    }
                } else {
                    for fd in fds {
                        fn_call(AttachmentId::notification(self, fd));
                    }
                }

                Ok(WaitEvent::Notification)
            }
            Err(ReactorWaitError::Interrupt) => Ok(WaitEvent::Interrupt),
            Err(ReactorWaitError::InsufficientPermissions) => {
                fail!(from self, with WaitSetWaitError::InsufficientPermissions,
                    "{msg} due to insufficient permissions.");
            }
            Err(ReactorWaitError::UnknownError) => {
                fail!(from self, with WaitSetWaitError::InternalError,
                    "{msg} due to an internal error.");
            }
        }
    }

    /// Returns the capacity of the [`WaitSet`]
    pub fn capacity(&self) -> usize {
        self.reactor.capacity()
    }

    /// Returns the number of attachments.
    pub fn len(&self) -> usize {
        self.reactor.len()
    }

    /// Returns true if the [`WaitSet`] has no attachments, otherwise false.
    pub fn is_empty(&self) -> bool {
        self.reactor.is_empty()
    }

    fn attach_to_reactor<'waitset, 'attachment, T: SynchronousMultiplexing + Debug>(
        &'waitset self,
        attachment: &'attachment T,
    ) -> Result<<Service::Reactor as Reactor>::Guard<'waitset, 'attachment>, WaitSetAttachmentError>
    {
        let msg = "Unable to attach object to internal reactor";

        match self.reactor.attach(attachment) {
            Ok(guard) => Ok(guard),
            Err(ReactorAttachError::AlreadyAttached) => {
                fail!(from self, with WaitSetAttachmentError::AlreadyAttached,
                    "{msg} {:?} since it is already attached.", attachment);
            }
            Err(ReactorAttachError::CapacityExceeded) => {
                fail!(from self, with WaitSetAttachmentError::AlreadyAttached,
                    "{msg} {:?} since it would exceed the capacity of {} of the waitset.",
                    attachment, self.capacity());
            }
            Err(ReactorAttachError::UnknownError(e)) => {
                fail!(from self, with WaitSetAttachmentError::InternalError,
                    "{msg} {:?} due to an internal error (error code = {})", attachment, e);
            }
        }
    }

    fn attach_to_timer<'waitset>(
        &'waitset self,
        timeout: Duration,
    ) -> Result<TimerGuard<'waitset>, WaitSetAttachmentError> {
        let msg = "Unable to attach timeout to underlying Timer";

        match self.timer.cyclic(timeout) {
            Ok(guard) => Ok(guard),
            Err(e) => {
                fail!(from self, with WaitSetAttachmentError::InternalError,
                    "{msg} since the timeout could not be attached to the underlying timer due to ({:?}).", e);
            }
        }
    }
}
