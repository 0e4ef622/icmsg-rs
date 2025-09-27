use bt_hci::{FromHciBytes, cmd::{self, AsyncCmd, Cmd}, param};
use bt_hci::cmd::SyncCmd;
use bt_hci::cmd::info::*;
use bt_hci::cmd::le::*;
use bt_hci::cmd::status::*;
use bt_hci::cmd::link_control::*;
use bt_hci::cmd::controller_baseband::*;
use nrf_sdc::vendor::*;

pub type CmdErr = cmd::Error<nrf_sdc::Error>;

macro_rules! dispatch_cmd {
    ($ctrl:expr, $opcode:expr, $payload:expr, [ $($items:tt)* ]) => {{
        let mut __matched = false;
        dispatch_cmd!(@munch __matched, $ctrl, $opcode, $payload, $($items)*);
        if __matched { Ok(()) } else { Err(cmd::Error::Hci(param::Error::UNSUPPORTED)) }
    }};

    (@munch $done:ident, $ctrl:expr, $opcode:expr, $payload:expr,
        @async ( $ty:path ) ; $($rest:tt)*
    ) => {{
        if !$done && $opcode == < $ty as Cmd >::OPCODE {
            let params =
                <<$ty as Cmd>::Params as FromHciBytes>
                ::from_hci_bytes_complete($payload)
                .map_err(|_| cmd::Error::Hci(param::Error::INVALID_HCI_PARAMETERS))?;
            let cmd_val = <$ty as From<<$ty as Cmd>::Params>>::from(params);

            match AsyncCmd::exec(&cmd_val, $ctrl).await {
                Ok(()) => {}
                Err(e) => return Err(e),
            }

            $done = true;
        }
        dispatch_cmd!(@munch $done, $ctrl, $opcode, $payload, $($rest)*);
    }};

    (@munch $done:ident, $ctrl:expr, $opcode:expr, $payload:expr,
        $ty:path ; $($rest:tt)*
    ) => {{
        if !$done && $opcode == < $ty as Cmd >::OPCODE {
            let params =
                <<$ty as Cmd>::Params as FromHciBytes>
                ::from_hci_bytes_complete($payload)
                .map_err(|_| cmd::Error::Hci(param::Error::INVALID_HCI_PARAMETERS))?;
            let cmd_val = <$ty as From<<$ty as Cmd>::Params>>::from(params);

            match SyncCmd::exec(&cmd_val, $ctrl).await {
                Ok(_) => {}
                Err(e) => return Err(e),
            }

            $done = true;
        }
        dispatch_cmd!(@munch $done, $ctrl, $opcode, $payload, $($rest)*);
    }};

    (@munch $done:ident, $ctrl:expr, $opcode:expr, $payload:expr,) => {};
}
pub(crate) use dispatch_cmd;

pub async fn exec_cmd_by_opcode<'d, E>(
    ctrl: &crate::sdc::SoftdeviceController<'d>,
    opcode: bt_hci::cmd::Opcode,
    payload: &[u8],
) -> Result<(), CmdErr>
where
    E: core::fmt::Debug,
{
    dispatch_cmd!(ctrl, opcode, payload, [

        // §7.1 Link Control
        Disconnect;
        @async(ReadRemoteVersionInformation);

        // §7.3 Controller & Baseband
        Reset;
        SetEventMask;
        ReadTransmitPowerLevel;
        SetControllerToHostFlowControl;
        HostBufferSize;
        SetEventMaskPage2;
        ReadAuthenticatedPayloadTimeout;
        WriteAuthenticatedPayloadTimeout;
        HostNumberOfCompletedPackets;

        // §7.4 Informational params
        ReadLocalVersionInformation;
        ReadLocalSupportedCmds;
        ReadLocalSupportedFeatures;
        ReadBdAddr;

        // §7.5 Status params
        ReadRssi;

        // §7.8 LE Controller (legacy + extended)
        LeSetAdvParams;
        LeReadAdvPhysicalChannelTxPower;
        LeSetAdvData;
        LeSetScanResponseData;
        LeSetAdvEnable;
        LeSetScanParams;
        LeSetScanEnable;
        @async(LeCreateConn);

        LeSetExtAdvParams;
        LeSetExtAdvParamsV2;
        LeReadMaxAdvDataLength;
        LeReadNumberOfSupportedAdvSets;
        LeRemoveAdvSet;
        LeClearAdvSets;
        LeSetPeriodicAdvParams;
        LeSetPeriodicAdvParamsV2;
        LeSetPeriodicAdvEnable;
        LeSetExtScanEnable;
        @async(LePeriodicAdvCreateSync);
        LePeriodicAdvCreateSyncCancel;
        LePeriodicAdvTerminateSync;
        LeAddDeviceToPeriodicAdvList;
        LeRemoveDeviceFromPeriodicAdvList;
        LeClearPeriodicAdvList;
        LeReadPeriodicAdvListSize;
        LeSetPeriodicAdvSyncTransferParams;
        LeSetDefaultPeriodicAdvSyncTransferParams;

        LeSetEventMask;
        LeReadBufferSize;
        LeReadLocalSupportedFeatures;
        LeSetRandomAddr;
        LeCreateConnCancel;
        LeReadFilterAcceptListSize;
        LeClearFilterAcceptList;
        LeAddDeviceToFilterAcceptList;
        LeRemoveDeviceFromFilterAcceptList;
        @async(LeConnUpdate);
        LeSetHostChannelClassification;
        LeReadChannelMap;
        @async(LeReadRemoteFeatures);
        LeEncrypt;
        LeRand;
        @async(LeEnableEncryption);
        LeLongTermKeyRequestReply;
        LeLongTermKeyRequestNegativeReply;
        LeReadSupportedStates;
        LeTestEnd;
        LeSetDataLength;
        LeReadSuggestedDefaultDataLength;
        LeWriteSuggestedDefaultDataLength;
        LeAddDeviceToResolvingList;
        LeRemoveDeviceFromResolvingList;
        LeClearResolvingList;
        LeReadResolvingListSize;
        LeSetAddrResolutionEnable;
        LeSetResolvablePrivateAddrTimeout;
        LeReadMaxDataLength;
        LeReadPhy;
        LeSetDefaultPhy;
        @async(LeSetPhy);
        LeSetAdvSetRandomAddr;
        LeReadTransmitPower;
        LeReadRfPathCompensation;
        LeWriteRfPathCompensation;
        LeSetPrivacyMode;
        LeSetConnectionlessCteTransmitEnable;
        LeConnCteResponseEnable;
        LeReadAntennaInformation;
        LeSetPeriodicAdvReceiveEnable;
        LePeriodicAdvSyncTransfer;
        LePeriodicAdvSetInfoTransfer;
        @async(LeRequestPeerSca);
        LeEnhancedReadTransmitPowerLevel;
        @async(LeReadRemoteTransmitPowerLevel);
        LeSetPathLossReportingParams;
        LeSetPathLossReportingEnable;
        LeSetTransmitPowerReportingEnable;
        LeSetDataRelatedAddrChanges;
        LeSetHostFeature;
        LeSetHostFeatureV2;

        // Extra LE impls in the fragment:
        // LeSetExtAdvData;
        // LeSetExtScanResponseData;
        // LeSetExtAdvEnable;
        // LeSetPeriodicAdvData;
        // LeSetExtScanParams;
        // @async(LeExtCreateConn);
        // LeSetConnectionlessCteTransmitParams;
        // LeSetConnCteTransmitParams;
        // @async(LeExtCreateConnV2);
        // LeSetPeriodicAdvSubeventData;
        // LeSetPeriodicAdvResponseData;
        // LeSetPeriodicSyncSubevent;

        // Vendor-specific (Zephyr/Nordic)
        ZephyrReadVersionInfo;
        ZephyrReadSupportedCommands;
        ZephyrWriteBdAddr;
        ZephyrReadKeyHierarchyRoots;
        ZephyrReadChipTemp;
        ZephyrWriteTxPower;
        ZephyrReadTxPower;

        NordicLlpmModeSet;
        NordicConnUpdate;
        NordicConnEventExtend;
        NordicQosConnEventReportEnable;
        NordicEventLengthSet;
        NordicPeriodicAdvEventLengthSet;
        NordicPeripheralLatencyModeSet;
        NordicWriteRemoteTxPower;
        NordicSetAdvRandomness;
        NordicCompatModeWindowOffsetSet;
        NordicQosChannelSurveyEnable;
        NordicSetPowerControlRequestParams;
        NordicReadAverageRssi;
        NordicCentralAclEventSpacingSet;
        NordicGetNextConnEventCounter;
        NordicAllowParallelConnectionEstablishments;
        NordicMinValOfMaxAclTxPayloadSet;
        NordicIsoReadTxTimestamp;
        NordicBigReservedTimeSet;
        NordicCigReservedTimeSet;
        NordicCisSubeventLengthSet;
        NordicScanChannelMapSet;
        NordicScanAcceptExtAdvPacketsSet;
        NordicSetRolePriority;
        NordicSetEventStartTask;
        NordicConnAnchorPointUpdateEventReportEnable;

        ZephyrReadStaticAddrs;
    ])
}
