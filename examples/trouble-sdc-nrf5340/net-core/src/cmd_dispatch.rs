use bt_hci::cmd;
use bt_hci::cmd::CmdReturnBuf;
use bt_hci::cmd::controller_baseband::*;
use bt_hci::cmd::info::*;
use bt_hci::cmd::le::*;
use bt_hci::cmd::link_control::*;
use bt_hci::cmd::status::*;
use bt_hci::event::{CommandComplete, EventParams};
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{raw::NoopRawMutex, Mutex};
use nrf_sdc::vendor::*;

pub type CmdErr = cmd::Error<nrf_sdc::Error>;

macro_rules! dispatch_cmd {
    (
        $send:expr, $ctrl:expr, $opcode:expr, $payload:expr,
        [ $( $(@ $async:tt)? $ty:ty ),* $(,)? ]
    ) => {{
        match $opcode {
            $(
                < $ty as bt_hci::cmd::Cmd >::OPCODE =>
                    dispatch_cmd!(@arm $ctrl, $payload, { $($async)? }, $ty, $send),
            )*
            _ => Err(bt_hci::cmd::Error::Hci(bt_hci::param::Error::UNSUPPORTED)),
        }
    }};

    // async -> ignore sender; return None
    (@arm $ctrl:expr, $payload:expr, {async}, $ty:path, $_sender:expr) => {{
        let params =
            <<$ty as bt_hci::cmd::Cmd>::Params as bt_hci::FromHciBytes>
            ::from_hci_bytes_complete($payload)
            .map_err(|_| bt_hci::cmd::Error::Hci(bt_hci::param::Error::INVALID_HCI_PARAMETERS))?;
        let cmd_val = <$ty as From<<$ty as bt_hci::cmd::Cmd>::Params>>::from(params);
        bt_hci::cmd::AsyncCmd::exec(&cmd_val, $ctrl).await
    }};

    // sync -> build CommandComplete event and send it via $sender; return None
    (@arm $ctrl:expr, $payload:expr, {}, $ty:path, $sender:expr) => {{
        let params =
            <<$ty as bt_hci::cmd::Cmd>::Params as bt_hci::FromHciBytes>
            ::from_hci_bytes_complete($payload)
            .map_err(|_| bt_hci::cmd::Error::Hci(bt_hci::param::Error::INVALID_HCI_PARAMETERS))?;
        let cmd_val = <$ty as From<<$ty as bt_hci::cmd::Cmd>::Params>>::from(params);

        // Execute sync command and get the parsed Return (POD / byte-backed).
        let ret = bt_hci::cmd::SyncCmd::exec(&cmd_val, $ctrl).await;

        // Sizes/constants
        const RLEN: usize = < $ty as bt_hci::cmd::SyncCmd >::ReturnBuf::LEN;
        const HEAD: usize = 7; // 1 pkt + 1 event + 1 len + 1 num + 2 opcode + 1 status
        let param_len: u8 = (1 + 2 + 1 + RLEN) as u8;

        // Allocate full controller->host event buffer on the stack.
        let mut buf = [0u8; HEAD + RLEN];

        // PacketKind::Event (0x04)
        buf[0] = bt_hci::PacketKind::Event as u8;

        buf[1] = CommandComplete::EVENT_CODE;

        // Parameter length
        buf[2] = param_len;

        // Num HCI command packets = 1
        buf[3] = 1;

        // Opcode (little-endian)
        let op16 = < $ty as bt_hci::cmd::Cmd >::OPCODE.into_inner();
        buf[4] = (op16 & 0xFF) as u8;
        buf[5] = (op16 >> 8) as u8;

        match ret {
            Ok(ret) => {
                buf[6] = bt_hci::param::Status::SUCCESS.into_inner();

                // Return parameters RLEN bytes: copy from the parsed Return value.
                unsafe {
                    // TODO this is a sketchy hack around the lack of WriteHci for command
                    // responses
                    let src = core::slice::from_raw_parts((&ret as *const _ as *const u8), RLEN);
                    buf[HEAD .. HEAD + RLEN].copy_from_slice(src);
                }
            }
            Err(bt_hci::cmd::Error::Hci(e)) => {
                buf[6] = e.to_status().into_inner();
            }
            Err(e) => {
                return Err(e);
            }
        }

        defmt::info!("[cmd_dispatch] Sending {} bytes: {=[u8]:x}", buf.len(), &buf[..]);
        $sender.lock(|x| x.borrow_mut().send(&buf)).map_err(|_| bt_hci::cmd::Error::Io(nrf_sdc::Error::EIO))
    }};

    (@arm $_c:expr, $_p:expr, {$bad:tt}, $_ty:path, $_s:expr) => {
        compile_error!("unknown marker after '@' — use '@async' or omit it")
    };
}
pub(crate) use dispatch_cmd;

use crate::IpcNotify;
use crate::icmsg_config::ALIGN;

pub async fn exec_cmd_by_opcode<'d, E>(
	send: &'static Mutex<NoopRawMutex, RefCell<icmsg::Sender<IpcNotify<'static>, ALIGN>>>,
    ctrl: &crate::sdc::SoftdeviceController<'d>,
    opcode: bt_hci::cmd::Opcode,
    payload: &[u8],
) -> Result<(), CmdErr>
where
    E: core::fmt::Debug,
{
    dispatch_cmd!(send, ctrl, opcode, payload, [
        // §7.1 Link Control
        Disconnect,
        @async ReadRemoteVersionInformation,

        // §7.3 Controller & Baseband
        Reset,
        SetEventMask,
        ReadTransmitPowerLevel,
        SetControllerToHostFlowControl,
        HostBufferSize,
        SetEventMaskPage2,
        ReadAuthenticatedPayloadTimeout,
        WriteAuthenticatedPayloadTimeout,
        HostNumberOfCompletedPackets,

        // §7.4 Informational params
        ReadLocalVersionInformation,
        ReadLocalSupportedCmds,
        ReadLocalSupportedFeatures,
        ReadBdAddr,

        // §7.5 Status params
        ReadRssi,

        // §7.8 LE Controller (legacy + extended)
        LeSetAdvParams,
        LeReadAdvPhysicalChannelTxPower,
        LeSetAdvData,
        LeSetScanResponseData,
        LeSetAdvEnable,
        LeSetScanParams,
        LeSetScanEnable,
        @async LeCreateConn,

        LeSetExtAdvParams,
        LeSetExtAdvParamsV2,
        LeReadMaxAdvDataLength,
        LeReadNumberOfSupportedAdvSets,
        LeRemoveAdvSet,
        LeClearAdvSets,
        LeSetPeriodicAdvParams,
        LeSetPeriodicAdvParamsV2,
        LeSetPeriodicAdvEnable,
        LeSetExtScanEnable,
        @async LePeriodicAdvCreateSync,
        LePeriodicAdvCreateSyncCancel,
        LePeriodicAdvTerminateSync,
        LeAddDeviceToPeriodicAdvList,
        LeRemoveDeviceFromPeriodicAdvList,
        LeClearPeriodicAdvList,
        LeReadPeriodicAdvListSize,
        LeSetPeriodicAdvSyncTransferParams,
        LeSetDefaultPeriodicAdvSyncTransferParams,

        LeSetEventMask,
        LeReadBufferSize,
        LeReadLocalSupportedFeatures,
        LeSetRandomAddr,
        LeCreateConnCancel,
        LeReadFilterAcceptListSize,
        LeClearFilterAcceptList,
        LeAddDeviceToFilterAcceptList,
        LeRemoveDeviceFromFilterAcceptList,
        @async LeConnUpdate,
        LeSetHostChannelClassification,
        LeReadChannelMap,
        @async LeReadRemoteFeatures,
        LeEncrypt,
        LeRand,
        @async LeEnableEncryption,
        LeLongTermKeyRequestReply,
        LeLongTermKeyRequestNegativeReply,
        LeReadSupportedStates,
        // LeTestEnd,
        LeSetDataLength,
        LeReadSuggestedDefaultDataLength,
        LeWriteSuggestedDefaultDataLength,
        LeAddDeviceToResolvingList,
        LeRemoveDeviceFromResolvingList,
        LeClearResolvingList,
        LeReadResolvingListSize,
        LeSetAddrResolutionEnable,
        LeSetResolvablePrivateAddrTimeout,
        LeReadMaxDataLength,
        LeReadPhy,
        LeSetDefaultPhy,
        @async LeSetPhy,
        LeSetAdvSetRandomAddr,
        LeReadTransmitPower,
        LeReadRfPathCompensation,
        LeWriteRfPathCompensation,
        LeSetPrivacyMode,
        LeSetConnectionlessCteTransmitEnable,
        LeConnCteResponseEnable,
        LeReadAntennaInformation,
        LeSetPeriodicAdvReceiveEnable,
        LePeriodicAdvSyncTransfer,
        LePeriodicAdvSetInfoTransfer,
        @async LeRequestPeerSca,
        LeEnhancedReadTransmitPowerLevel,
        @async LeReadRemoteTransmitPowerLevel,
        LeSetPathLossReportingParams,
        LeSetPathLossReportingEnable,
        LeSetTransmitPowerReportingEnable,
        LeSetDataRelatedAddrChanges,
        LeSetHostFeature,
        LeSetHostFeatureV2,

        // Extra LE impls in the fragment:
        // LeSetExtAdvData,
        // LeSetExtScanResponseData,
        // LeSetExtAdvEnable,
        // LeSetPeriodicAdvData,
        // LeSetExtScanParams,
        // @async LeExtCreateConn,
        // LeSetConnectionlessCteTransmitParams,
        // LeSetConnCteTransmitParams,
        // @async LeExtCreateConnV2,
        // LeSetPeriodicAdvSubeventData,
        // LeSetPeriodicAdvResponseData,
        // LeSetPeriodicSyncSubevent,

        // Vendor-specific (Zephyr/Nordic)
        ZephyrReadVersionInfo,
        ZephyrReadSupportedCommands,
        ZephyrWriteBdAddr,
        ZephyrReadKeyHierarchyRoots,
        ZephyrReadChipTemp,
        ZephyrWriteTxPower,
        ZephyrReadTxPower,

        NordicLlpmModeSet,
        NordicConnUpdate,
        NordicConnEventExtend,
        NordicQosConnEventReportEnable,
        NordicEventLengthSet,
        NordicPeriodicAdvEventLengthSet,
        NordicPeripheralLatencyModeSet,
        NordicWriteRemoteTxPower,
        NordicSetAdvRandomness,
        NordicCompatModeWindowOffsetSet,
        NordicQosChannelSurveyEnable,
        NordicSetPowerControlRequestParams,
        NordicReadAverageRssi,
        NordicCentralAclEventSpacingSet,
        NordicGetNextConnEventCounter,
        NordicAllowParallelConnectionEstablishments,
        NordicMinValOfMaxAclTxPayloadSet,
        NordicIsoReadTxTimestamp,
        NordicBigReservedTimeSet,
        NordicCigReservedTimeSet,
        NordicCisSubeventLengthSet,
        NordicScanChannelMapSet,
        NordicScanAcceptExtAdvPacketsSet,
        NordicSetRolePriority,
        NordicSetEventStartTask,
        NordicConnAnchorPointUpdateEventReportEnable,

        ZephyrReadStaticAddrs,
    ])
}
