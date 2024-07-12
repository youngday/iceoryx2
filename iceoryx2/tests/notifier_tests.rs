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

#[generic_tests::define]
mod notifier {
    use std::collections::HashSet;

    use iceoryx2::{
        node::NodeBuilder,
        port::notifier::{NotifierCreateError, NotifierNotifyError},
        service::{service_name::ServiceName, Service},
    };
    use iceoryx2_bb_posix::unique_system_id::UniqueSystemId;
    use iceoryx2_bb_testing::assert_that;

    fn generate_name() -> ServiceName {
        ServiceName::new(&format!(
            "listener_tests_{}",
            UniqueSystemId::new().unwrap().value()
        ))
        .unwrap()
    }

    #[test]
    fn create_error_display_works<S: Service>() {
        assert_that!(
            format!("{}", NotifierCreateError::ExceedsMaxSupportedNotifiers), eq "NotifierCreateError::ExceedsMaxSupportedNotifiers");
    }

    #[test]
    fn notify_error_display_works<S: Service>() {
        assert_that!(
            format!("{}", NotifierNotifyError::EventIdOutOfBounds), eq "NotifierNotifyError::EventIdOutOfBounds");
    }

    #[test]
    fn id_is_unique<Sut: Service>() {
        let service_name = generate_name();
        let node = NodeBuilder::new().create::<Sut>().unwrap();
        const MAX_LISTENERS: usize = 8;

        let sut = node
            .service_builder(service_name.clone())
            .event()
            .max_listeners(MAX_LISTENERS)
            .create()
            .unwrap();

        let mut listeners = vec![];
        let mut listener_id_set = HashSet::new();

        for _ in 0..MAX_LISTENERS {
            let listener = sut.listener_builder().create().unwrap();
            assert_that!(listener_id_set.insert(listener.id()), eq true);
            listeners.push(listener);
        }
    }

    #[instantiate_tests(<iceoryx2::service::zero_copy::Service>)]
    mod zero_copy {}

    #[instantiate_tests(<iceoryx2::service::process_local::Service>)]
    mod process_local {}
}