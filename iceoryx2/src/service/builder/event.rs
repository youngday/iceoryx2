// Copyright (c) 2023 Contributors to the Eclipse Foundation
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
//! See [`crate::service`]
//!
pub use crate::port::event_id::EventId;
use crate::service::messaging_pattern::MessagingPattern;
use crate::service::port_factory::event;
use crate::service::*;
use crate::service::{self, dynamic_config::event::DynamicConfigSettings};
use iceoryx2_bb_elementary::enum_gen;
use iceoryx2_bb_log::{fail, fatal_panic};
use iceoryx2_bb_posix::adaptive_wait::AdaptiveWaitBuilder;
use iceoryx2_cal::dynamic_storage::DynamicStorageCreateError;

use super::ServiceState;

/// Failures that can occur when an existing [`MessagingPattern::Event`] [`Service`] shall be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventOpenError {
    DoesNotExist,
    PermissionDenied,
    EventInCorruptedState,
    IncompatibleMessagingPattern,
    InternalFailure,
    HangsInCreation,
    DoesNotSupportRequestedAmountOfNotifiers,
    DoesNotSupportRequestedAmountOfListeners,
    DoesNotSupportRequestedMaxEventId,
    UnableToOpenDynamicServiceInformation,
}

impl std::fmt::Display for EventOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::write!(f, "{}::{:?}", std::stringify!(Self), self)
    }
}

impl std::error::Error for EventOpenError {}

/// Failures that can occur when a new [`MessagingPattern::Event`] [`Service`] shall be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventCreateError {
    Corrupted,
    InternalFailure,
    IsBeingCreatedByAnotherInstance,
    AlreadyExists,
    PermissionDenied,
    UnableToCreateStaticServiceInformation,
    OldConnectionsStillActive,
}

impl std::fmt::Display for EventCreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::write!(f, "{}::{:?}", std::stringify!(Self), self)
    }
}

impl std::error::Error for EventCreateError {}

enum_gen! {
    /// Failures that can occur when a [`MessagingPattern::Event`] [`Service`] shall be opened or
    /// created.
    EventOpenOrCreateError
  mapping:
    EventOpenError,
    EventCreateError
}

impl std::fmt::Display for EventOpenOrCreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::write!(f, "{}::{:?}", std::stringify!(Self), self)
    }
}

impl std::error::Error for EventOpenOrCreateError {}

/// Builder to create new [`MessagingPattern::Event`] based [`Service`]s
///
/// # Example
///
/// See [`crate::service`]
#[derive(Debug)]
pub struct Builder<ServiceType: service::Service> {
    base: builder::BuilderWithServiceType<ServiceType>,
    verify_max_notifiers: bool,
    verify_max_listeners: bool,
    verify_event_id_max_value: bool,
}

impl<ServiceType: service::Service> Builder<ServiceType> {
    pub(crate) fn new(base: builder::BuilderWithServiceType<ServiceType>) -> Self {
        let mut new_self = Self {
            base,
            verify_max_notifiers: false,
            verify_max_listeners: false,
            verify_event_id_max_value: false,
        };

        new_self.base.service_config.messaging_pattern = MessagingPattern::Event(
            static_config::event::StaticConfig::new(new_self.base.global_config.as_ref()),
        );

        new_self
    }

    fn config_details(&mut self) -> &mut static_config::event::StaticConfig {
        match self.base.service_config.messaging_pattern {
            MessagingPattern::Event(ref mut v) => v,
            _ => {
                fatal_panic!(from self, "This should never happen! Accessing wrong messaging pattern in Event builder!");
            }
        }
    }

    /// If the [`Service`] is created it defines how many [`crate::port::notifier::Notifier`] shall
    /// be supported at most. If an existing [`Service`] is opened it defines how many
    /// [`crate::port::notifier::Notifier`] must be at least supported.
    pub fn event_id_max_value(mut self, value: usize) -> Self {
        self.config_details().event_id_max_value = value;
        self.verify_event_id_max_value = true;
        self
    }

    /// If the [`Service`] is created it defines how many [`crate::port::notifier::Notifier`] shall
    /// be supported at most. If an existing [`Service`] is opened it defines how many
    /// [`crate::port::notifier::Notifier`] must be at least supported.
    pub fn max_notifiers(mut self, value: usize) -> Self {
        self.config_details().max_notifiers = value;
        self.verify_max_notifiers = true;
        self
    }

    /// If the [`Service`] is created it defines how many [`crate::port::listener::Listener`] shall
    /// be supported at most. If an existing [`Service`] is opened it defines how many
    /// [`crate::port::listener::Listener`] must be at least supported.
    pub fn max_listeners(mut self, value: usize) -> Self {
        self.config_details().max_listeners = value;
        self.verify_max_listeners = true;
        self
    }

    /// If the [`Service`] exists, it will be opened otherwise a new [`Service`] will be
    /// created.
    pub fn open_or_create(
        mut self,
    ) -> Result<event::PortFactory<ServiceType>, EventOpenOrCreateError> {
        let msg = "Unable to open or create event service";

        match self.base.is_service_available() {
            Ok(Some(_)) => Ok(self.open()?),
            Ok(None) => match self.create_impl() {
                Ok(factory) => Ok(factory),
                Err(EventCreateError::AlreadyExists)
                | Err(EventCreateError::IsBeingCreatedByAnotherInstance) => Ok(self.open()?),
                Err(e) => Err(e.into()),
            },
            Err(ServiceState::IsBeingCreatedByAnotherInstance) => Ok(self.open()?),
            Err(ServiceState::Corrupted) => {
                fail!(from self, with EventOpenOrCreateError::EventOpenError(EventOpenError::EventInCorruptedState),
                    "{} since the event is in a corrupted state.", msg);
            }
            Err(ServiceState::IncompatibleMessagingPattern) => {
                fail!(from self, with EventOpenOrCreateError::EventOpenError(EventOpenError::IncompatibleMessagingPattern),
                    "{} since the services messaging pattern does not match.", msg);
            }
            Err(ServiceState::PermissionDenied) => {
                fail!(from self, with EventOpenOrCreateError::EventOpenError(EventOpenError::PermissionDenied),
                    "{} due to insufficient permissions.", msg);
            }
        }
    }

    /// Opens an existing [`Service`].
    pub fn open(mut self) -> Result<event::PortFactory<ServiceType>, EventOpenError> {
        let msg = "Unable to open event service";

        let mut adaptive_wait = fail!(from self, when AdaptiveWaitBuilder::new().create(),
                                        with EventOpenError::InternalFailure,
                                        "{} since the adaptive wait could not be created.", msg);

        loop {
            match self.base.is_service_available() {
                Ok(None) => {
                    fail!(from self, with EventOpenError::DoesNotExist,
                        "{} since the event does not exist.", msg);
                }
                Ok(Some((static_config, static_storage))) => {
                    let static_config = self.verify_service_properties(&static_config)?;

                    let dynamic_config = Rc::new(
                        fail!(from self, when self.base.open_dynamic_config_storage(),
                            with EventOpenError::UnableToOpenDynamicServiceInformation,
                            "{} since the dynamic service informations could not be opened.", msg),
                    );

                    self.base.service_config.messaging_pattern =
                        MessagingPattern::Event(static_config);

                    return Ok(event::PortFactory::new(ServiceType::from_state(
                        service::ServiceState::new(
                            self.base.service_config,
                            self.base.global_config,
                            dynamic_config,
                            static_storage,
                        ),
                    )));
                }
                Err(ServiceState::IsBeingCreatedByAnotherInstance) => {
                    let timeout = fail!(from self, when adaptive_wait.wait(),
                                        with EventOpenError::InternalFailure,
                                        "{} since the adaptive wait failed.", msg);

                    if timeout > self.base.global_config.global.service.creation_timeout {
                        fail!(from self, with EventOpenError::HangsInCreation,
                            "{} since the service hangs while being created, max timeout for service creation of {:?} exceeded. Waited for {:?} but the state did not change.",
                            msg, self.base.global_config.global.service.creation_timeout, timeout);
                    }
                }
                Err(ServiceState::PermissionDenied) => {
                    fail!(from self, with EventOpenError::PermissionDenied,
                        "{} due to insufficient permissions.", msg);
                }
                Err(ServiceState::IncompatibleMessagingPattern) => {
                    fail!(from self, with EventOpenError::PermissionDenied,
                        "{} since the services messaging pattern does not match.", msg);
                }
                Err(ServiceState::Corrupted) => {
                    fail!(from self, with EventOpenError::EventInCorruptedState,
                        "{} since the event is in a corrupted state.", msg);
                }
            }
        }
    }

    fn create_impl(&mut self) -> Result<event::PortFactory<ServiceType>, EventCreateError> {
        self.adjust_properties_to_meaningful_values();

        let msg = "Unable to create event service";

        match self.base.is_service_available() {
            Ok(None) => {
                let static_config = match self.base.create_static_config_storage() {
                    Ok(c) => c,
                    Err(StaticStorageCreateError::AlreadyExists) => {
                        fail!(from self, with EventCreateError::AlreadyExists,
                           "{} since the service already exists.", msg);
                    }
                    Err(StaticStorageCreateError::Creation) => {
                        fail!(from self, with EventCreateError::IsBeingCreatedByAnotherInstance,
                            "{} since the service is being created by another instance.", msg);
                    }
                    Err(e) => {
                        fail!(from self, with EventCreateError::UnableToCreateStaticServiceInformation,
                            "{} since the static service information could not be created ({:?}).", msg, e);
                    }
                };

                let event_config = self.base.service_config.event();

                let dynamic_config_setting = DynamicConfigSettings {
                    number_of_listeners: event_config.max_listeners,
                    number_of_notifiers: event_config.max_notifiers,
                };

                let dynamic_config = match self.base.create_dynamic_config_storage(
                    dynamic_config::MessagingPattern::Event(
                        dynamic_config::event::DynamicConfig::new(&dynamic_config_setting),
                    ),
                    dynamic_config::event::DynamicConfig::memory_size(&dynamic_config_setting),
                ) {
                    Ok(c) => Rc::new(c),
                    Err(DynamicStorageCreateError::AlreadyExists) => {
                        fail!(from self, with EventCreateError::OldConnectionsStillActive,
                            "{} since there are still active Listeners or Notifiers.", msg);
                    }
                    Err(e) => {
                        fail!(from self, with EventCreateError::InternalFailure,
                            "{} since the dynamic service segment could not be created ({:?}).", msg, e);
                    }
                };

                let service_config = fail!(from self, when ServiceType::ConfigSerializer::serialize(&self.base.service_config),
                                            with EventCreateError::Corrupted,
                                            "{} since the configuration could not be serialized.", msg);

                // only unlock the static details when the service is successfully created
                let mut unlocked_static_details = fail!(from self, when static_config.unlock(service_config.as_slice()),
                            with EventCreateError::Corrupted,
                            "{} since the configuration could not be written to the static storage.", msg);

                unlocked_static_details.release_ownership();

                Ok(event::PortFactory::new(ServiceType::from_state(
                    service::ServiceState::new(
                        self.base.service_config.clone(),
                        self.base.global_config.clone(),
                        dynamic_config,
                        unlocked_static_details,
                    ),
                )))
            }
            Ok(Some(_)) | Err(ServiceState::IncompatibleMessagingPattern) => {
                fail!(from self, with EventCreateError::AlreadyExists,
                    "{} since the service already exists.", msg);
            }
            Err(ServiceState::PermissionDenied) => {
                fail!(from self, with EventCreateError::PermissionDenied,
                    "{} due to possible insufficient permissions to access the underlying service details.", msg);
            }
            Err(ServiceState::Corrupted) => {
                fail!(from self, with EventCreateError::Corrupted,
                    "{} since a service in a corrupted state already exists. A cleanup of the service constructs may help,", msg);
            }
            Err(ServiceState::IsBeingCreatedByAnotherInstance) => {
                fail!(from self, with EventCreateError::IsBeingCreatedByAnotherInstance,
                    "{} since the service is being created by another instance.", msg);
            }
        }
    }

    /// Creates a new [`Service`].
    pub fn create(mut self) -> Result<event::PortFactory<ServiceType>, EventCreateError> {
        self.create_impl()
    }

    fn adjust_properties_to_meaningful_values(&mut self) {
        let origin = format!("{:?}", self);
        let settings = self.base.service_config.event_mut();

        if settings.max_notifiers == 0 {
            warn!(from origin, "Setting the maximum amount of notifiers to 0 is not supported. Adjust it to 1, the smallest supported value.");
            settings.max_notifiers = 1;
        }

        if settings.max_listeners == 0 {
            warn!(from origin, "Setting the maximum amount of listeners to 0 is not supported. Adjust it to 1, the smallest supported value.");
            settings.max_listeners = 1;
        }
    }

    fn verify_service_properties(
        &self,
        existing_settings: &static_config::StaticConfig,
    ) -> Result<static_config::event::StaticConfig, EventOpenError> {
        let msg = "Unable to open event";

        let required_settings = self.base.service_config.event();
        let existing_settings = match &existing_settings.messaging_pattern {
            MessagingPattern::Event(ref v) => v,
            p => {
                fail!(from self, with EventOpenError::IncompatibleMessagingPattern,
                "{} since a service with the messaging pattern {:?} exists but MessagingPattern::Event is required.", msg, p);
            }
        };

        if self.verify_max_notifiers
            && existing_settings.max_notifiers < required_settings.max_notifiers
        {
            fail!(from self, with EventOpenError::DoesNotSupportRequestedAmountOfNotifiers,
                "{} since the event supports only {} notifiers but a support of {} notifiers was requested.",
                msg, existing_settings.max_notifiers, required_settings.max_notifiers);
        }

        if self.verify_max_listeners
            && existing_settings.max_listeners < required_settings.max_listeners
        {
            fail!(from self, with EventOpenError::DoesNotSupportRequestedAmountOfListeners,
                "{} since the event supports only {} listeners but a support of {} listeners was requested.",
                msg, existing_settings.max_notifiers, required_settings.max_listeners);
        }

        if self.verify_event_id_max_value
            && existing_settings.event_id_max_value < required_settings.event_id_max_value
        {
            fail!(from self, with EventOpenError::DoesNotSupportRequestedMaxEventId,
                "{} since the event supports only EventIds with a value of at most {} a support of {} was requested.",
                msg, existing_settings.event_id_max_value, required_settings.event_id_max_value);
        }

        Ok(*existing_settings)
    }
}
