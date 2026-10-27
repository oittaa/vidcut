use std::ffi::c_void;

use super::CudaFrames;
use super::burn::pool_frame;
use super::mix::{frame_u32, kernel_of, stride_u64};
use super::share::{Cuda, check, cuda_context, in_context};
use crate::error::Result;
use crate::export::place::{GpuPlane, Layer};
use crate::ffmpeg::{ffi, format, frame};

const PTX: &str = r#"
.version 7.0
.target sm_52
.address_size 64
.visible .entry vidcut_place(
	.param .align 8 .b8 vidcut_place_param_0[872]
)
{
	.local .align 8 .b8 	__local_depot0[24];
	.reg .b64 	%SP;
	.reg .b64 	%SPL;
	.reg .pred 	%p<125>;
	.reg .b32 	%r<1160>;
	.reg .b64 	%rd<126>;
	mov.b64 	%SPL, __local_depot0;
	add.u64 	%rd1, %SPL, 0;
	add.u64 	%rd125, %SPL, 16;
	mov.u32 	%r215, %ctaid.x;
	mov.u32 	%r216, %ntid.x;
	mov.u32 	%r217, %tid.x;
	mad.lo.s32 	%r1, %r215, %r216, %r217;
	mov.u32 	%r218, %ctaid.y;
	mov.u32 	%r219, %ntid.y;
	mov.u32 	%r220, %tid.y;
	mad.lo.s32 	%r221, %r218, %r219, %r220;
	ld.param.v2.b32 	{%r222, %r223}, [vidcut_place_param_0+80];
	setp.ge.u32 	%p1, %r1, %r222;
	setp.ge.u32 	%p2, %r221, %r223;
	or.pred 	%p3, %p1, %p2;
	@%p3 bra 	$L__BB0_318;
	mov.b64 	%rd63, vidcut_place_param_0;
	ld.param.v2.b32 	{%r2, %r3}, [vidcut_place_param_0+88];
	add.s32 	%r1157, %r2, 1;
	mul.wide.u32 	%rd64, %r2, 8;
	add.s64 	%rd3, %rd63, %rd64;
	ld.param.b64 	%rd65, [%rd3+16];
	cvta.to.global.u64 	%rd66, %rd65;
	ld.param.b64 	%rd68, [%rd3+32];
	ld.param.b64 	%rd71, [vidcut_place_param_0];
	cvta.to.global.u64 	%rd72, %rd71;
	st.local.b64 	[%rd125], 0;
	ld.param.v2.b32 	{%r225, %r226}, [vidcut_place_param_0+104];
	setp.lt.u32 	%p4, %r1, %r225;
	ld.param.v2.b32 	{%r227, %r228}, [vidcut_place_param_0+112];
	setp.ge.u32 	%p5, %r1, %r227;
	setp.lt.u32 	%p6, %r221, %r226;
	or.pred 	%p7, %p4, %p6;
	or.pred 	%p8, %p7, %p5;
	setp.ge.u32 	%p9, %r221, %r228;
	mov.pred 	%p124, 0;
	mov.b32 	%r1067, 0f00000000;
	or.pred 	%p10, %p8, %p9;
	@%p10 bra 	$L__BB0_11;
	ld.param.b32 	%r7, [vidcut_place_param_0+120];
	setp.eq.b32 	%p11, %r7, 0;
	mov.b32 	%r1066, 0f3F800000;
	mov.pred 	%p123, 0;
	@%p11 bra 	$L__BB0_10;
	cvt.rn.f32.u32 	%r229, %r1;
	add.rn.f32 	%r230, %r229, 0f3F000000;
	ld.param.b32 	%r231, [vidcut_place_param_0+140];
	mul.rn.f32 	%r5, %r230, %r231;
	cvt.rn.f32.u32 	%r232, %r221;
	add.rn.f32 	%r233, %r232, 0f3F000000;
	ld.param.b32 	%r234, [vidcut_place_param_0+144];
	mul.rn.f32 	%r6, %r233, %r234;
	add.s32 	%r1064, %r7, -1;
	add.s64 	%rd124, %rd63, 332;
	mov.b32 	%r1066, 0f3F800000;
	bra.uni 	$L__BB0_4;
$L__BB0_8:
	mul.rn.f32 	%r252, %r8, %r14;
	sub.rn.f32 	%r253, %r12, %r252;
	mul.rn.f32 	%r254, %r9, %r14;
	sub.rn.f32 	%r255, %r13, %r254;
	mul.rn.f32 	%r256, %r8, %r17;
	sub.rn.f32 	%r257, %r15, %r256;
	mul.rn.f32 	%r258, %r9, %r17;
	sub.rn.f32 	%r259, %r16, %r258;
	mul.rn.f32 	%r260, %r253, %r253;
	mul.rn.f32 	%r261, %r255, %r255;
	add.rn.f32 	%r262, %r260, %r261;
	sqrt.rn.f32 	%r263, %r262;
	div.rn.f32 	%r264, %r263, %r11;
	mul.rn.f32 	%r265, %r257, %r257;
	mul.rn.f32 	%r266, %r259, %r259;
	add.rn.f32 	%r267, %r265, %r266;
	sqrt.rn.f32 	%r268, %r267;
	div.rn.f32 	%r269, %r268, %r11;
	ld.param.b32 	%r270, [%rd124+-40];
	sub.rn.f32 	%r271, %r14, %r270;
	div.rn.f32 	%r272, %r271, %r264;
	add.rn.f32 	%r273, %r272, 0f3F000000;
	max.f32 	%r274, %r273, 0f00000000;
	min.f32 	%r275, %r274, 0f3F800000;
	mul.rn.f32 	%r276, %r1066, %r275;
	ld.param.b32 	%r277, [%rd124+-32];
	sub.rn.f32 	%r278, %r277, %r14;
	div.rn.f32 	%r279, %r278, %r264;
	add.rn.f32 	%r280, %r279, 0f3F000000;
	max.f32 	%r281, %r280, 0f00000000;
	min.f32 	%r282, %r281, 0f3F800000;
	mul.rn.f32 	%r283, %r276, %r282;
	ld.param.b32 	%r284, [%rd124+-36];
	sub.rn.f32 	%r285, %r17, %r284;
	div.rn.f32 	%r286, %r285, %r269;
	add.rn.f32 	%r287, %r286, 0f3F000000;
	max.f32 	%r288, %r287, 0f00000000;
	min.f32 	%r289, %r288, 0f3F800000;
	mul.rn.f32 	%r290, %r283, %r289;
	ld.param.b32 	%r291, [%rd124+-28];
	sub.rn.f32 	%r292, %r291, %r17;
	div.rn.f32 	%r293, %r292, %r269;
	add.rn.f32 	%r294, %r293, 0f3F000000;
	max.f32 	%r295, %r294, 0f00000000;
	min.f32 	%r296, %r295, 0f3F800000;
	mul.rn.f32 	%r18, %r290, %r296;
	ld.param.b32 	%r333, [%rd124];
	mul.rn.f32 	%r334, %r1065, %r333;
	mul.rn.f32 	%r1066, %r18, %r334;
	setp.leu.f32 	%p123, %r1066, 0f00000000;
$L__BB0_9:
	setp.ne.b32 	%p17, %r1064, 0;
	not.pred 	%p18, %p123;
	add.s32 	%r1064, %r1064, -1;
	add.s64 	%rd124, %rd124, 76;
	and.pred 	%p19, %p18, %p17;
	@!%p19 bra 	$L__BB0_10;
$L__BB0_4:
	ld.param.b32 	%r8, [%rd124+-48];
	mul.rn.f32 	%r235, %r5, %r8;
	ld.param.b32 	%r9, [%rd124+-44];
	mul.rn.f32 	%r236, %r6, %r9;
	add.rn.f32 	%r237, %r235, %r236;
	add.rn.f32 	%r10, %r237, 0f3F800000;
	abs.f32 	%r11, %r10;
	shl.b32 	%r238, %r10, 1;
	setp.gt.u32 	%p12, %r238, -16777217;
	setp.lt.f32 	%p13, %r11, 0f2B8CBCCC;
	or.pred 	%p14, %p12, %p13;
	mov.pred 	%p123, -1;
	@%p14 bra 	$L__BB0_9;
	ld.param.b32 	%r12, [%rd124+-72];
	mul.rn.f32 	%r239, %r5, %r12;
	ld.param.b32 	%r13, [%rd124+-68];
	mul.rn.f32 	%r240, %r6, %r13;
	add.rn.f32 	%r241, %r239, %r240;
	ld.param.b32 	%r242, [%rd124+-64];
	add.rn.f32 	%r243, %r242, %r241;
	div.rn.f32 	%r14, %r243, %r10;
	ld.param.b32 	%r15, [%rd124+-60];
	mul.rn.f32 	%r244, %r5, %r15;
	ld.param.b32 	%r16, [%rd124+-56];
	mul.rn.f32 	%r245, %r6, %r16;
	add.rn.f32 	%r246, %r244, %r245;
	ld.param.b32 	%r247, [%rd124+-52];
	add.rn.f32 	%r248, %r247, %r246;
	div.rn.f32 	%r17, %r248, %r10;
	shl.b32 	%r249, %r14, 1;
	shl.b32 	%r250, %r17, 1;
	max.u32 	%r251, %r249, %r250;
	setp.gt.u32 	%p15, %r251, -16777217;
	@%p15 bra 	$L__BB0_9;
	ld.param.b32 	%r19, [%rd124+-8];
	ld.param.b32 	%r297, [%rd124+-4];
	min.f32 	%r298, %r19, %r297;
	setp.le.f32 	%p16, %r298, 0f00000000;
	mov.b32 	%r1065, 0f3F800000;
	@%p16 bra 	$L__BB0_8;
	ld.param.b32 	%r299, [%rd124+-24];
	sub.rn.f32 	%r300, %r299, %r14;
	ld.param.b32 	%r301, [%rd124+-16];
	sub.rn.f32 	%r302, %r14, %r301;
	max.f32 	%r303, %r300, %r302;
	max.f32 	%r304, %r303, 0f00000000;
	div.rn.f32 	%r305, %r304, %r19;
	ld.param.b32 	%r306, [%rd124+-20];
	sub.rn.f32 	%r307, %r306, %r17;
	ld.param.b32 	%r308, [%rd124+-12];
	sub.rn.f32 	%r309, %r17, %r308;
	max.f32 	%r310, %r307, %r309;
	max.f32 	%r311, %r310, 0f00000000;
	div.rn.f32 	%r312, %r311, %r297;
	sub.rn.f32 	%r313, %r305, %r312;
	abs.f32 	%r314, %r313;
	mov.b32 	%r315, 0f3F000000;
	sub.rn.f32 	%r316, %r315, %r314;
	max.f32 	%r317, %r316, 0f00000000;
	add.rn.f32 	%r318, %r317, %r317;
	max.f32 	%r319, %r305, %r312;
	mul.rn.f32 	%r320, %r318, %r318;
	mul.rn.f32 	%r321, %r320, 0f3F000000;
	mul.rn.f32 	%r322, %r321, 0f3E800000;
	add.rn.f32 	%r323, %r319, %r322;
	mov.b32 	%r324, 0f3F800000;
	sub.rn.f32 	%r325, %r324, %r323;
	div.rn.f32 	%r326, %r325, 0f3F600000;
	max.f32 	%r327, %r326, 0f00000000;
	min.f32 	%r328, %r327, 0f3F800000;
	mul.rn.f32 	%r329, %r328, %r328;
	add.rn.f32 	%r330, %r328, %r328;
	mov.b32 	%r331, 0f40400000;
	sub.rn.f32 	%r332, %r331, %r330;
	mul.rn.f32 	%r1065, %r329, %r332;
	bra.uni 	$L__BB0_8;
$L__BB0_10:
	selp.f32 	%r1067, 0f00000000, %r1066, %p123;
	not.pred 	%p124, %p123;
$L__BB0_11:
	mad.lo.s32 	%r4, %r3, %r2, %r3;
	not.pred 	%p20, %p124;
	setp.eq.b32 	%p121, %r1157, 0;
	@%p20 bra 	$L__BB0_307;
	cvt.u64.u32 	%rd2, %r2;
	ld.param.b32 	%r20, [vidcut_place_param_0+124];
	setp.ne.b32 	%p21, %r20, 0;
	setp.ne.b32 	%p122, %r3, 1;
	@!%p21 bra 	$L__BB0_13;
	ld.param.b32 	%r335, [vidcut_place_param_0+120];
	add.s32 	%r336, %r335, -1;
	mul.wide.u32 	%rd75, %r336, 76;
	add.s64 	%rd19, %rd63, %rd75;
	ld.param.b64 	%rd76, [vidcut_place_param_0+48];
	cvta.to.global.u64 	%rd20, %rd76;
	ld.param.b32 	%r52, [vidcut_place_param_0+192];
	setp.eq.b32 	%p22, %r52, 0;
	ld.param.b32 	%r53, [vidcut_place_param_0+188];
	mov.b32 	%r1099, 0f00000000;
	cvt.rn.f32.u32 	%r1060, %r1;
	cvt.rn.f32.u32 	%r1061, %r221;
	mov.b32 	%r1088, %r1099;
	@%p22 bra 	$L__BB0_69;
	ld.param.b64 	%rd21, [vidcut_place_param_0+64];
	cvt.rn.f32.u32 	%r54, %r53;
	cvt.rn.f32.u32 	%r55, %r52;
	ld.param.b32 	%r56, [vidcut_place_param_0+140];
	mul.rn.f32 	%r57, %r56, %r1060;
	ld.param.b32 	%r58, [vidcut_place_param_0+144];
	mul.rn.f32 	%r59, %r58, %r1061;
	ld.param.b32 	%r60, [vidcut_place_param_0+164];
	ld.param.v2.b32 	{%r61, %r62}, [vidcut_place_param_0+168];
	ld.param.b32 	%r63, [vidcut_place_param_0+196];
	ld.param.v2.b32 	{%r64, %r65}, [vidcut_place_param_0+200];
	ld.param.v2.b32 	{%r66, %r337}, [vidcut_place_param_0+208];
	max.f32 	%r67, %r337, %r65;
	ld.param.v2.b32 	{%r68, %r338}, [vidcut_place_param_0+176];
	ld.param.b32 	%r339, [vidcut_place_param_0+216];
	max.f32 	%r69, %r339, %r66;
	ld.param.b32 	%r340, [vidcut_place_param_0+184];
	add.s32 	%r70, %r340, -1;
	add.s32 	%r71, %r338, -1;
	ld.param.v2.b32 	{%r72, %r73}, [vidcut_place_param_0+96];
	mov.b32 	%r1082, 0;
	mov.b32 	%r1088, 0f00000000;
	setp.eq.b32 	%p23, %r53, 0;
	bra.uni 	$L__BB0_51;
$L__BB0_68:
	add.s32 	%r1082, %r1082, 1;
	setp.eq.b32 	%p28, %r1082, %r52;
	@%p28 bra 	$L__BB0_69;
$L__BB0_51:
	@%p23 bra 	$L__BB0_68;
	cvt.rn.f32.u32 	%r341, %r1082;
	add.rn.f32 	%r342, %r341, 0f3F000000;
	div.rn.f32 	%r343, %r342, %r55;
	mul.rn.f32 	%r344, %r343, %r58;
	add.rn.f32 	%r74, %r59, %r344;
	ld.param.b32 	%r75, [%rd19+284];
	ld.param.b32 	%r345, [%rd19+288];
	mul.rn.f32 	%r76, %r345, %r74;
	mov.b32 	%r1083, 0;
	bra.uni 	$L__BB0_53;
$L__BB0_65:
	ld.global.b8 	%r1087, [%rd25];
$L__BB0_66:
	shr.u32 	%r403, %r1087, %r72;
	and.b32 	%r404, %r403, %r73;
	cvt.rn.f32.u32 	%r405, %r404;
	mul.rn.f32 	%r406, %r79, %r405;
	add.rn.f32 	%r407, %r85, %r406;
	mul.rn.f32 	%r408, %r82, %r84;
	mul.rn.f32 	%r409, %r80, %r407;
	add.rn.f32 	%r410, %r408, %r409;
	add.rn.f32 	%r1088, %r1088, %r410;
$L__BB0_67:
	add.s32 	%r1083, %r1083, 1;
	setp.eq.b32 	%p27, %r53, %r1083;
	@%p27 bra 	$L__BB0_68;
$L__BB0_53:
	cvt.rn.f32.u32 	%r346, %r1083;
	add.rn.f32 	%r347, %r346, 0f3F000000;
	div.rn.f32 	%r348, %r347, %r54;
	mul.rn.f32 	%r349, %r56, %r348;
	add.rn.f32 	%r77, %r57, %r349;
	mul.rn.f32 	%r350, %r75, %r77;
	add.rn.f32 	%r351, %r76, %r350;
	add.rn.f32 	%r78, %r351, 0f3F800000;
	abs.f32 	%r352, %r78;
	shl.b32 	%r353, %r78, 1;
	setp.gt.u32 	%p24, %r353, -16777217;
	setp.lt.f32 	%p25, %r352, 0f2B8CBCCC;
	or.pred 	%p26, %p24, %p25;
	@%p26 bra 	$L__BB0_67;
	ld.param.b32 	%r354, [%rd19+260];
	mul.rn.f32 	%r355, %r77, %r354;
	ld.param.b32 	%r356, [%rd19+264];
	mul.rn.f32 	%r357, %r74, %r356;
	add.rn.f32 	%r358, %r355, %r357;
	ld.param.b32 	%r359, [%rd19+268];
	add.rn.f32 	%r360, %r359, %r358;
	div.rn.f32 	%r361, %r360, %r78;
	ld.param.b32 	%r362, [%rd19+272];
	mul.rn.f32 	%r363, %r77, %r362;
	ld.param.b32 	%r364, [%rd19+276];
	mul.rn.f32 	%r365, %r74, %r364;
	add.rn.f32 	%r366, %r363, %r365;
	ld.param.b32 	%r367, [%rd19+280];
	add.rn.f32 	%r368, %r367, %r366;
	div.rn.f32 	%r369, %r368, %r78;
	mul.rn.f32 	%r370, %r361, %r62;
	add.rn.f32 	%r371, %r60, %r370;
	div.rn.f32 	%r372, %r371, %r63;
	add.rn.f32 	%r373, %r372, 0fBF000000;
	max.f32 	%r374, %r373, %r65;
	min.f32 	%r375, %r374, %r67;
	mul.rn.f32 	%r376, %r369, %r68;
	add.rn.f32 	%r377, %r61, %r376;
	div.rn.f32 	%r378, %r377, %r64;
	add.rn.f32 	%r379, %r378, 0fBF000000;
	max.f32 	%r380, %r379, %r66;
	min.f32 	%r381, %r380, %r69;
	cvt.rmi.f32.f32 	%r382, %r375;
	cvt.rmi.f32.f32 	%r383, %r381;
	sub.rn.f32 	%r79, %r375, %r382;
	cvt.rzi.u32.f32 	%r384, %r383;
	add.s32 	%r385, %r384, 1;
	min.u32 	%r386, %r385, %r70;
	cvt.u64.u32 	%rd79, %r384;
	mad.lo.s64 	%rd80, %rd21, %rd79, %rd20;
	cvt.rzi.u32.f32 	%r387, %r382;
	add.s32 	%r388, %r387, 1;
	min.u32 	%r389, %r388, %r71;
	mul.lo.s32 	%r390, %r3, %r387;
	cvt.u64.u32 	%rd81, %r390;
	add.s64 	%rd22, %rd80, %rd81;
	mul.lo.s32 	%r392, %r389, %r3;
	cvt.u64.u32 	%rd82, %r392;
	@%p122 bra 	$L__BB0_55;
	ld.global.b8 	%r1084, [%rd22];
	bra.uni 	$L__BB0_57;
$L__BB0_55:
	ld.global.b16 	%r1084, [%rd22];
$L__BB0_57:
	cvt.u64.u32 	%rd77, %r386;
	mov.b32 	%r391, 0f3F800000;
	sub.rn.f32 	%r81, %r391, %r79;
	add.s64 	%rd23, %rd80, %rd82;
	shr.u32 	%r393, %r1084, %r72;
	and.b32 	%r394, %r393, %r73;
	cvt.rn.f32.u32 	%r395, %r394;
	@%p122 bra 	$L__BB0_58;
	ld.global.b8 	%r1085, [%rd23];
	bra.uni 	$L__BB0_60;
$L__BB0_58:
	ld.global.b16 	%r1085, [%rd23];
$L__BB0_60:
	sub.rn.f32 	%r80, %r381, %r383;
	mad.lo.s64 	%rd78, %rd21, %rd77, %rd20;
	mul.rn.f32 	%r83, %r81, %r395;
	add.s64 	%rd24, %rd78, %rd81;
	shr.u32 	%r396, %r1085, %r72;
	and.b32 	%r397, %r396, %r73;
	cvt.rn.f32.u32 	%r398, %r397;
	mul.rn.f32 	%r399, %r79, %r398;
	@%p122 bra 	$L__BB0_61;
	ld.global.b8 	%r1086, [%rd24];
	bra.uni 	$L__BB0_63;
$L__BB0_61:
	ld.global.b16 	%r1086, [%rd24];
$L__BB0_63:
	add.s64 	%rd25, %rd78, %rd82;
	sub.rn.f32 	%r82, %r391, %r80;
	add.rn.f32 	%r84, %r83, %r399;
	shr.u32 	%r400, %r1086, %r72;
	and.b32 	%r401, %r400, %r73;
	cvt.rn.f32.u32 	%r402, %r401;
	mul.rn.f32 	%r85, %r81, %r402;
	@!%p122 bra 	$L__BB0_65;
	ld.global.b16 	%r1087, [%rd25];
	bra.uni 	$L__BB0_66;
$L__BB0_69:
	ld.param.b64 	%rd83, [vidcut_place_param_0+56];
	cvta.to.global.u64 	%rd26, %rd83;
	shl.b32 	%r87, %r3, 1;
	ld.param.b32 	%r88, [vidcut_place_param_0+232];
	setp.eq.b32 	%p29, %r88, 0;
	ld.param.b32 	%r89, [vidcut_place_param_0+228];
	cvt.u64.u32 	%rd123, %r3;
	mov.b32 	%r1100, %r1099;
	@%p29 bra 	$L__BB0_101;
	ld.param.b64 	%rd27, [vidcut_place_param_0+72];
	cvt.rn.f32.u32 	%r90, %r89;
	cvt.rn.f32.u32 	%r91, %r88;
	ld.param.b32 	%r92, [vidcut_place_param_0+140];
	mul.rn.f32 	%r93, %r92, %r1060;
	ld.param.b32 	%r94, [vidcut_place_param_0+144];
	mul.rn.f32 	%r95, %r94, %r1061;
	ld.param.b32 	%r96, [vidcut_place_param_0+164];
	ld.param.v2.b32 	{%r97, %r98}, [vidcut_place_param_0+168];
	ld.param.b32 	%r99, [vidcut_place_param_0+236];
	ld.param.v2.b32 	{%r100, %r101}, [vidcut_place_param_0+240];
	ld.param.v2.b32 	{%r102, %r413}, [vidcut_place_param_0+248];
	max.f32 	%r103, %r413, %r101;
	ld.param.b32 	%r104, [vidcut_place_param_0+176];
	ld.param.b32 	%r414, [vidcut_place_param_0+256];
	max.f32 	%r105, %r414, %r102;
	ld.param.b32 	%r415, [vidcut_place_param_0+224];
	add.s32 	%r106, %r415, -1;
	ld.param.b32 	%r416, [vidcut_place_param_0+220];
	add.s32 	%r107, %r416, -1;
	ld.param.v2.b32 	{%r108, %r109}, [vidcut_place_param_0+96];
	mov.b32 	%r1089, 0;
	mov.b32 	%r1100, 0f00000000;
	setp.eq.b32 	%p30, %r89, 0;
	mov.b32 	%r1099, %r1100;
	bra.uni 	$L__BB0_71;
$L__BB0_100:
	add.s32 	%r1089, %r1089, 1;
	setp.eq.b32 	%p35, %r1089, %r88;
	@%p35 bra 	$L__BB0_101;
$L__BB0_71:
	@%p30 bra 	$L__BB0_100;
	cvt.rn.f32.u32 	%r417, %r1089;
	add.rn.f32 	%r418, %r417, 0f3F000000;
	div.rn.f32 	%r419, %r418, %r91;
	mul.rn.f32 	%r420, %r419, %r94;
	add.rn.f32 	%r110, %r95, %r420;
	ld.param.b32 	%r111, [%rd19+284];
	ld.param.b32 	%r421, [%rd19+288];
	mul.rn.f32 	%r112, %r421, %r110;
	mov.b32 	%r1090, 0;
	bra.uni 	$L__BB0_73;
$L__BB0_97:
	ld.global.b16 	%r1098, [%rd35];
$L__BB0_98:
	shr.u32 	%r489, %r1098, %r108;
	and.b32 	%r490, %r489, %r109;
	cvt.rn.f32.u32 	%r491, %r490;
	mul.rn.f32 	%r492, %r115, %r491;
	add.rn.f32 	%r493, %r124, %r492;
	mul.rn.f32 	%r494, %r118, %r123;
	mul.rn.f32 	%r495, %r116, %r493;
	add.rn.f32 	%r496, %r494, %r495;
	add.rn.f32 	%r1099, %r1099, %r496;
	shr.u32 	%r497, %r1094, %r108;
	and.b32 	%r498, %r497, %r109;
	cvt.rn.f32.u32 	%r499, %r498;
	mul.rn.f32 	%r500, %r115, %r499;
	add.rn.f32 	%r501, %r121, %r500;
	mul.rn.f32 	%r502, %r118, %r120;
	mul.rn.f32 	%r503, %r116, %r501;
	add.rn.f32 	%r504, %r502, %r503;
	add.rn.f32 	%r1100, %r1100, %r504;
$L__BB0_99:
	add.s32 	%r1090, %r1090, 1;
	setp.eq.b32 	%p34, %r89, %r1090;
	@%p34 bra 	$L__BB0_100;
$L__BB0_73:
	cvt.rn.f32.u32 	%r422, %r1090;
	add.rn.f32 	%r423, %r422, 0f3F000000;
	div.rn.f32 	%r424, %r423, %r90;
	mul.rn.f32 	%r425, %r92, %r424;
	add.rn.f32 	%r113, %r93, %r425;
	mul.rn.f32 	%r426, %r111, %r113;
	add.rn.f32 	%r427, %r112, %r426;
	add.rn.f32 	%r114, %r427, 0f3F800000;
	abs.f32 	%r428, %r114;
	shl.b32 	%r429, %r114, 1;
	setp.gt.u32 	%p31, %r429, -16777217;
	setp.lt.f32 	%p32, %r428, 0f2B8CBCCC;
	or.pred 	%p33, %p31, %p32;
	@%p33 bra 	$L__BB0_99;
	ld.param.b32 	%r430, [%rd19+260];
	mul.rn.f32 	%r431, %r113, %r430;
	ld.param.b32 	%r432, [%rd19+264];
	mul.rn.f32 	%r433, %r110, %r432;
	add.rn.f32 	%r434, %r431, %r433;
	ld.param.b32 	%r435, [%rd19+268];
	add.rn.f32 	%r436, %r435, %r434;
	div.rn.f32 	%r437, %r436, %r114;
	ld.param.b32 	%r438, [%rd19+272];
	mul.rn.f32 	%r439, %r113, %r438;
	ld.param.b32 	%r440, [%rd19+276];
	mul.rn.f32 	%r441, %r110, %r440;
	add.rn.f32 	%r442, %r439, %r441;
	ld.param.b32 	%r443, [%rd19+280];
	add.rn.f32 	%r444, %r443, %r442;
	div.rn.f32 	%r445, %r444, %r114;
	mul.rn.f32 	%r446, %r437, %r98;
	add.rn.f32 	%r447, %r96, %r446;
	div.rn.f32 	%r448, %r447, %r99;
	add.rn.f32 	%r449, %r448, 0fBF000000;
	max.f32 	%r450, %r449, %r101;
	min.f32 	%r451, %r450, %r103;
	mul.rn.f32 	%r452, %r445, %r104;
	add.rn.f32 	%r453, %r97, %r452;
	div.rn.f32 	%r454, %r453, %r100;
	add.rn.f32 	%r455, %r454, 0fBF000000;
	max.f32 	%r456, %r455, %r102;
	min.f32 	%r457, %r456, %r105;
	cvt.rmi.f32.f32 	%r458, %r451;
	cvt.rmi.f32.f32 	%r459, %r457;
	cvt.rzi.u32.f32 	%r460, %r459;
	cvt.u64.u32 	%rd86, %r460;
	mad.lo.s64 	%rd87, %rd27, %rd86, %rd26;
	cvt.rzi.u32.f32 	%r463, %r458;
	mul.lo.s32 	%r466, %r87, %r463;
	cvt.u64.u32 	%rd88, %r466;
	add.s64 	%rd28, %rd87, %rd88;
	@%p122 bra 	$L__BB0_75;
	ld.global.b8 	%r1091, [%rd28];
	bra.uni 	$L__BB0_77;
$L__BB0_75:
	ld.global.b16 	%r1091, [%rd28];
$L__BB0_77:
	add.s32 	%r464, %r463, 1;
	min.u32 	%r465, %r464, %r107;
	mul.lo.s32 	%r468, %r465, %r87;
	cvt.u64.u32 	%rd89, %r468;
	add.s64 	%rd29, %rd87, %rd89;
	@%p122 bra 	$L__BB0_78;
	ld.global.b8 	%r1092, [%rd29];
	bra.uni 	$L__BB0_80;
$L__BB0_78:
	ld.global.b16 	%r1092, [%rd29];
$L__BB0_80:
	add.s32 	%r461, %r460, 1;
	min.u32 	%r462, %r461, %r106;
	cvt.u64.u32 	%rd84, %r462;
	mad.lo.s64 	%rd85, %rd27, %rd84, %rd26;
	add.s64 	%rd30, %rd85, %rd88;
	@%p122 bra 	$L__BB0_81;
	ld.global.b8 	%r1093, [%rd30];
	bra.uni 	$L__BB0_83;
$L__BB0_81:
	ld.global.b16 	%r1093, [%rd30];
$L__BB0_83:
	shr.u32 	%r469, %r1091, %r108;
	shr.u32 	%r472, %r1092, %r108;
	add.s64 	%rd31, %rd85, %rd89;
	@%p122 bra 	$L__BB0_84;
	ld.global.b8 	%r1094, [%rd31];
	bra.uni 	$L__BB0_86;
$L__BB0_84:
	ld.global.b16 	%r1094, [%rd31];
$L__BB0_86:
	sub.rn.f32 	%r115, %r451, %r458;
	and.b32 	%r470, %r469, %r109;
	and.b32 	%r473, %r472, %r109;
	shr.u32 	%r476, %r1093, %r108;
	add.s64 	%rd32, %rd28, %rd123;
	@%p122 bra 	$L__BB0_88;
	ld.global.b8 	%r1095, [%rd32];
	bra.uni 	$L__BB0_89;
$L__BB0_88:
	ld.global.b16 	%r1095, [%rd32];
$L__BB0_89:
	mov.b32 	%r467, 0f3F800000;
	sub.rn.f32 	%r117, %r467, %r115;
	cvt.rn.f32.u32 	%r471, %r470;
	cvt.rn.f32.u32 	%r474, %r473;
	and.b32 	%r477, %r476, %r109;
	shr.u32 	%r479, %r1095, %r108;
	and.b32 	%r480, %r479, %r109;
	cvt.rn.f32.u32 	%r481, %r480;
	add.s64 	%rd33, %rd29, %rd123;
	@%p122 bra 	$L__BB0_91;
	ld.global.b8 	%r1096, [%rd33];
	bra.uni 	$L__BB0_92;
$L__BB0_91:
	ld.global.b16 	%r1096, [%rd33];
$L__BB0_92:
	sub.rn.f32 	%r116, %r457, %r459;
	mul.rn.f32 	%r119, %r117, %r471;
	mul.rn.f32 	%r475, %r115, %r474;
	cvt.rn.f32.u32 	%r478, %r477;
	mul.rn.f32 	%r122, %r117, %r481;
	shr.u32 	%r482, %r1096, %r108;
	and.b32 	%r483, %r482, %r109;
	cvt.rn.f32.u32 	%r484, %r483;
	mul.rn.f32 	%r485, %r115, %r484;
	add.s64 	%rd34, %rd30, %rd123;
	@%p122 bra 	$L__BB0_94;
	ld.global.b8 	%r1097, [%rd34];
	bra.uni 	$L__BB0_95;
$L__BB0_94:
	ld.global.b16 	%r1097, [%rd34];
$L__BB0_95:
	sub.rn.f32 	%r118, %r467, %r116;
	add.rn.f32 	%r120, %r119, %r475;
	mul.rn.f32 	%r121, %r117, %r478;
	add.rn.f32 	%r123, %r122, %r485;
	shr.u32 	%r486, %r1097, %r108;
	and.b32 	%r487, %r486, %r109;
	cvt.rn.f32.u32 	%r488, %r487;
	mul.rn.f32 	%r124, %r117, %r488;
	add.s64 	%rd35, %rd31, %rd123;
	@%p122 bra 	$L__BB0_97;
	ld.global.b8 	%r1098, [%rd35];
	bra.uni 	$L__BB0_98;
$L__BB0_101:
	ld.param.b32 	%r127, [vidcut_place_param_0+132];
	ld.param.b32 	%r128, [vidcut_place_param_0+136];
	shl.b32 	%r129, %r1, %r2;
	add.s32 	%r507, %r127, -1;
	min.u32 	%r130, %r129, %r507;
	shl.b32 	%r131, %r221, %r2;
	add.s32 	%r508, %r128, -1;
	min.u32 	%r132, %r131, %r508;
	add.s32 	%r509, %r1, 1;
	shl.b32 	%r133, %r509, %r2;
	add.s32 	%r510, %r130, 1;
	setp.lt.u32 	%p36, %r133, %r510;
	min.u32 	%r511, %r133, %r127;
	selp.b32 	%r134, %r510, %r511, %p36;
	add.s32 	%r512, %r221, 1;
	shl.b32 	%r135, %r512, %r2;
	add.s32 	%r513, %r132, 1;
	setp.lt.u32 	%p37, %r135, %r513;
	min.u32 	%r514, %r135, %r128;
	selp.b32 	%r136, %r513, %r514, %p37;
	setp.le.u32 	%p38, %r136, %r132;
	mov.b32 	%r1118, 0f00000000;
	sub.s32 	%r1063, %r134, %r130;
	mov.b32 	%r1106, %r1118;
	@%p38 bra 	$L__BB0_126;
	ld.param.b64 	%rd90, [vidcut_place_param_0+16];
	cvta.to.global.u64 	%rd36, %rd90;
	ld.param.b64 	%rd37, [vidcut_place_param_0+32];
	ld.param.v2.b32 	{%r137, %r138}, [vidcut_place_param_0+96];
	and.b32 	%r139, %r1063, 3;
	sub.s32 	%r140, %r130, %r134;
	mul.lo.s32 	%r141, %r3, %r130;
	neg.s32 	%r142, %r139;
	shl.b32 	%r143, %r3, 2;
	mov.b32 	%r1106, 0f00000000;
	setp.le.u32 	%p39, %r134, %r130;
	setp.eq.b32 	%p40, %r139, 0;
	setp.gt.u32 	%p42, %r140, -4;
	mov.b32 	%r1101, %r132;
	bra.uni 	$L__BB0_103;
$L__BB0_125:
	add.s32 	%r1101, %r1101, 1;
	setp.eq.b32 	%p44, %r1101, %r136;
	@%p44 bra 	$L__BB0_126;
$L__BB0_103:
	@%p39 bra 	$L__BB0_125;
	cvt.u64.u32 	%rd91, %r1101;
	mad.lo.s64 	%rd38, %rd37, %rd91, %rd36;
	mov.b32 	%r1105, %r130;
	@%p40 bra 	$L__BB0_110;
	mov.b32 	%r1102, %r142;
	mov.b32 	%r1103, %r141;
	mov.b32 	%r1105, %r130;
	bra.uni 	$L__BB0_106;
$L__BB0_108:
	ld.global.b8 	%r1104, [%rd39];
$L__BB0_109:
	shr.u32 	%r515, %r1104, %r137;
	and.b32 	%r516, %r515, %r138;
	cvt.rn.f32.u32 	%r517, %r516;
	add.rn.f32 	%r1106, %r1106, %r517;
	add.s32 	%r1105, %r1105, 1;
	add.s32 	%r1103, %r1103, %r3;
	add.s32 	%r1102, %r1102, 1;
	setp.ne.b32 	%p41, %r1102, 0;
	@!%p41 bra 	$L__BB0_110;
$L__BB0_106:
	.pragma "nounroll";
	cvt.u64.u32 	%rd92, %r1103;
	add.s64 	%rd39, %rd38, %rd92;
	@!%p122 bra 	$L__BB0_108;
	ld.global.b16 	%r1104, [%rd39];
	bra.uni 	$L__BB0_109;
$L__BB0_110:
	@%p42 bra 	$L__BB0_125;
	sub.s32 	%r1108, %r134, %r1105;
	mul.lo.s32 	%r147, %r3, %r1105;
	add.s32 	%r144, %r147, %r3;
	add.s32 	%r518, %r1105, 2;
	mul.lo.s32 	%r145, %r3, %r518;
	add.s32 	%r519, %r1105, 3;
	mul.lo.s32 	%r146, %r3, %r519;
	mov.b32 	%r1107, 0;
	bra.uni 	$L__BB0_112;
$L__BB0_123:
	ld.global.b8 	%r1112, [%rd43];
$L__BB0_124:
	shr.u32 	%r533, %r1112, %r137;
	and.b32 	%r534, %r533, %r138;
	cvt.rn.f32.u32 	%r535, %r534;
	add.rn.f32 	%r1106, %r150, %r535;
	add.s32 	%r1108, %r1108, -4;
	add.s32 	%r1107, %r1107, %r143;
	setp.eq.b32 	%p43, %r1108, 0;
	@%p43 bra 	$L__BB0_125;
$L__BB0_112:
	add.s32 	%r520, %r147, %r1107;
	cvt.u64.u32 	%rd93, %r520;
	add.s64 	%rd40, %rd38, %rd93;
	@%p122 bra 	$L__BB0_114;
	ld.global.b8 	%r1109, [%rd40];
	bra.uni 	$L__BB0_115;
$L__BB0_114:
	ld.global.b16 	%r1109, [%rd40];
$L__BB0_115:
	shr.u32 	%r521, %r1109, %r137;
	and.b32 	%r522, %r521, %r138;
	cvt.rn.f32.u32 	%r523, %r522;
	add.rn.f32 	%r148, %r1106, %r523;
	add.s32 	%r524, %r144, %r1107;
	cvt.u64.u32 	%rd94, %r524;
	add.s64 	%rd41, %rd38, %rd94;
	@%p122 bra 	$L__BB0_116;
	ld.global.b8 	%r1110, [%rd41];
	bra.uni 	$L__BB0_118;
$L__BB0_116:
	ld.global.b16 	%r1110, [%rd41];
$L__BB0_118:
	shr.u32 	%r525, %r1110, %r137;
	and.b32 	%r526, %r525, %r138;
	cvt.rn.f32.u32 	%r527, %r526;
	add.rn.f32 	%r149, %r148, %r527;
	add.s32 	%r528, %r145, %r1107;
	cvt.u64.u32 	%rd95, %r528;
	add.s64 	%rd42, %rd38, %rd95;
	@%p122 bra 	$L__BB0_119;
	ld.global.b8 	%r1111, [%rd42];
	bra.uni 	$L__BB0_121;
$L__BB0_119:
	ld.global.b16 	%r1111, [%rd42];
$L__BB0_121:
	shr.u32 	%r529, %r1111, %r137;
	and.b32 	%r530, %r529, %r138;
	cvt.rn.f32.u32 	%r531, %r530;
	add.rn.f32 	%r150, %r149, %r531;
	add.s32 	%r532, %r146, %r1107;
	cvt.u64.u32 	%rd96, %r532;
	add.s64 	%rd43, %rd38, %rd96;
	@!%p122 bra 	$L__BB0_123;
	ld.global.b16 	%r1112, [%rd43];
	bra.uni 	$L__BB0_124;
$L__BB0_126:
	mul.lo.s32 	%r411, %r53, %r52;
	mul.lo.s32 	%r505, %r89, %r88;
	sub.s32 	%r536, %r136, %r132;
	add.s32 	%r539, %r127, 1;
	shr.u32 	%r540, %r539, 1;
	add.s32 	%r541, %r128, 1;
	shr.u32 	%r542, %r541, 1;
	shr.u32 	%r543, %r129, 1;
	add.s32 	%r544, %r540, -1;
	min.u32 	%r152, %r543, %r544;
	shr.u32 	%r545, %r131, 1;
	add.s32 	%r546, %r542, -1;
	min.u32 	%r1125, %r545, %r546;
	shr.u32 	%r547, %r133, 1;
	add.s32 	%r548, %r152, 1;
	setp.gt.u32 	%p45, %r547, %r152;
	min.u32 	%r549, %r547, %r540;
	selp.b32 	%r153, %r549, %r548, %p45;
	shr.u32 	%r550, %r135, 1;
	add.s32 	%r551, %r1125, 1;
	setp.gt.u32 	%p46, %r550, %r1125;
	min.u32 	%r552, %r550, %r542;
	selp.b32 	%r154, %r552, %r551, %p46;
	setp.le.u32 	%p47, %r154, %r1125;
	sub.s32 	%r1062, %r153, %r152;
	@%p47 bra 	$L__BB0_151;
	ld.param.b64 	%rd97, [vidcut_place_param_0+24];
	cvta.to.global.u64 	%rd44, %rd97;
	ld.param.b64 	%rd45, [vidcut_place_param_0+40];
	ld.param.v2.b32 	{%r155, %r156}, [vidcut_place_param_0+96];
	and.b32 	%r157, %r1062, 3;
	sub.s32 	%r158, %r152, %r153;
	mul.lo.s32 	%r553, %r3, %r152;
	shl.b32 	%r159, %r553, 1;
	neg.s32 	%r160, %r157;
	shl.b32 	%r161, %r3, 3;
	mov.b32 	%r1118, 0f00000000;
	setp.ge.u32 	%p48, %r152, %r153;
	setp.eq.b32 	%p49, %r157, 0;
	setp.gt.u32 	%p51, %r158, -4;
	mov.b32 	%r1113, %r1125;
	bra.uni 	$L__BB0_128;
$L__BB0_150:
	add.s32 	%r1113, %r1113, 1;
	setp.eq.b32 	%p53, %r1113, %r154;
	@%p53 bra 	$L__BB0_151;
$L__BB0_128:
	@%p48 bra 	$L__BB0_150;
	cvt.u64.u32 	%rd98, %r1113;
	mad.lo.s64 	%rd46, %rd45, %rd98, %rd44;
	mov.b32 	%r1117, %r152;
	@%p49 bra 	$L__BB0_135;
	mov.b32 	%r1114, %r160;
	mov.b32 	%r1115, %r159;
	mov.b32 	%r1117, %r152;
	bra.uni 	$L__BB0_131;
$L__BB0_133:
	ld.global.b8 	%r1116, [%rd47];
$L__BB0_134:
	shr.u32 	%r554, %r1116, %r155;
	and.b32 	%r555, %r554, %r156;
	cvt.rn.f32.u32 	%r556, %r555;
	add.rn.f32 	%r1118, %r1118, %r556;
	add.s32 	%r1117, %r1117, 1;
	add.s32 	%r1115, %r1115, %r87;
	add.s32 	%r1114, %r1114, 1;
	setp.ne.b32 	%p50, %r1114, 0;
	@!%p50 bra 	$L__BB0_135;
$L__BB0_131:
	.pragma "nounroll";
	cvt.u64.u32 	%rd99, %r1115;
	add.s64 	%rd47, %rd46, %rd99;
	@!%p122 bra 	$L__BB0_133;
	ld.global.b16 	%r1116, [%rd47];
	bra.uni 	$L__BB0_134;
$L__BB0_135:
	@%p51 bra 	$L__BB0_150;
	sub.s32 	%r1120, %r153, %r1117;
	shl.b32 	%r557, %r1117, 1;
	add.s32 	%r558, %r557, 2;
	mul.lo.s32 	%r162, %r3, %r558;
	add.s32 	%r559, %r557, 4;
	mul.lo.s32 	%r163, %r3, %r559;
	add.s32 	%r560, %r557, 6;
	mul.lo.s32 	%r164, %r3, %r560;
	mul.lo.s32 	%r165, %r87, %r1117;
	mov.b32 	%r1119, 0;
	bra.uni 	$L__BB0_137;
$L__BB0_148:
	ld.global.b8 	%r1124, [%rd51];
$L__BB0_149:
	shr.u32 	%r574, %r1124, %r155;
	and.b32 	%r575, %r574, %r156;
	cvt.rn.f32.u32 	%r576, %r575;
	add.rn.f32 	%r1118, %r168, %r576;
	add.s32 	%r1120, %r1120, -4;
	add.s32 	%r1119, %r1119, %r161;
	setp.eq.b32 	%p52, %r1120, 0;
	@%p52 bra 	$L__BB0_150;
$L__BB0_137:
	add.s32 	%r561, %r165, %r1119;
	cvt.u64.u32 	%rd100, %r561;
	add.s64 	%rd48, %rd46, %rd100;
	@%p122 bra 	$L__BB0_139;
	ld.global.b8 	%r1121, [%rd48];
	bra.uni 	$L__BB0_140;
$L__BB0_139:
	ld.global.b16 	%r1121, [%rd48];
$L__BB0_140:
	shr.u32 	%r562, %r1121, %r155;
	and.b32 	%r563, %r562, %r156;
	cvt.rn.f32.u32 	%r564, %r563;
	add.rn.f32 	%r166, %r1118, %r564;
	add.s32 	%r565, %r162, %r1119;
	cvt.u64.u32 	%rd101, %r565;
	add.s64 	%rd49, %rd46, %rd101;
	@%p122 bra 	$L__BB0_141;
	ld.global.b8 	%r1122, [%rd49];
	bra.uni 	$L__BB0_143;
$L__BB0_141:
	ld.global.b16 	%r1122, [%rd49];
$L__BB0_143:
	shr.u32 	%r566, %r1122, %r155;
	and.b32 	%r567, %r566, %r156;
	cvt.rn.f32.u32 	%r568, %r567;
	add.rn.f32 	%r167, %r166, %r568;
	add.s32 	%r569, %r163, %r1119;
	cvt.u64.u32 	%rd102, %r569;
	add.s64 	%rd50, %rd46, %rd102;
	@%p122 bra 	$L__BB0_144;
	ld.global.b8 	%r1123, [%rd50];
	bra.uni 	$L__BB0_146;
$L__BB0_144:
	ld.global.b16 	%r1123, [%rd50];
$L__BB0_146:
	shr.u32 	%r570, %r1123, %r155;
	and.b32 	%r571, %r570, %r156;
	cvt.rn.f32.u32 	%r572, %r571;
	add.rn.f32 	%r168, %r167, %r572;
	add.s32 	%r573, %r164, %r1119;
	cvt.u64.u32 	%rd103, %r573;
	add.s64 	%rd51, %rd46, %rd103;
	@!%p122 bra 	$L__BB0_148;
	ld.global.b16 	%r1124, [%rd51];
	bra.uni 	$L__BB0_149;
$L__BB0_151:
	mul.lo.s32 	%r537, %r536, %r1063;
	cvt.rn.f32.u32 	%r412, %r411;
	cvt.rn.f32.u32 	%r506, %r505;
	sub.s32 	%r577, %r154, %r1125;
	mul.lo.s32 	%r578, %r577, %r1062;
	mov.b32 	%r1130, 0f00000000;
	@%p47 bra 	$L__BB0_176;
	ld.param.b64 	%rd104, [vidcut_place_param_0+24];
	cvta.to.global.u64 	%rd105, %rd104;
	add.s64 	%rd52, %rd105, %rd123;
	ld.param.b64 	%rd53, [vidcut_place_param_0+40];
	ld.param.v2.b32 	{%r171, %r172}, [vidcut_place_param_0+96];
	and.b32 	%r173, %r1062, 3;
	sub.s32 	%r174, %r152, %r153;
	mul.lo.s32 	%r579, %r3, %r152;
	shl.b32 	%r175, %r579, 1;
	neg.s32 	%r176, %r173;
	shl.b32 	%r177, %r3, 3;
	mov.b32 	%r1130, 0f00000000;
	setp.ge.u32 	%p54, %r152, %r153;
	setp.eq.b32 	%p55, %r173, 0;
	setp.gt.u32 	%p57, %r174, -4;
	bra.uni 	$L__BB0_153;
$L__BB0_175:
	add.s32 	%r1125, %r1125, 1;
	setp.eq.b32 	%p59, %r1125, %r154;
	@%p59 bra 	$L__BB0_176;
$L__BB0_153:
	@%p54 bra 	$L__BB0_175;
	cvt.u64.u32 	%rd106, %r1125;
	mad.lo.s64 	%rd54, %rd53, %rd106, %rd52;
	mov.b32 	%r1129, %r152;
	@%p55 bra 	$L__BB0_160;
	mov.b32 	%r1126, %r176;
	mov.b32 	%r1127, %r175;
	mov.b32 	%r1129, %r152;
	bra.uni 	$L__BB0_156;
$L__BB0_158:
	ld.global.b8 	%r1128, [%rd55];
$L__BB0_159:
	shr.u32 	%r580, %r1128, %r171;
	and.b32 	%r581, %r580, %r172;
	cvt.rn.f32.u32 	%r582, %r581;
	add.rn.f32 	%r1130, %r1130, %r582;
	add.s32 	%r1129, %r1129, 1;
	add.s32 	%r1127, %r1127, %r87;
	add.s32 	%r1126, %r1126, 1;
	setp.ne.b32 	%p56, %r1126, 0;
	@!%p56 bra 	$L__BB0_160;
$L__BB0_156:
	.pragma "nounroll";
	cvt.u64.u32 	%rd107, %r1127;
	add.s64 	%rd55, %rd54, %rd107;
	@!%p122 bra 	$L__BB0_158;
	ld.global.b16 	%r1128, [%rd55];
	bra.uni 	$L__BB0_159;
$L__BB0_160:
	@%p57 bra 	$L__BB0_175;
	sub.s32 	%r1132, %r153, %r1129;
	shl.b32 	%r583, %r1129, 1;
	add.s32 	%r584, %r583, 2;
	mul.lo.s32 	%r178, %r3, %r584;
	add.s32 	%r585, %r583, 4;
	mul.lo.s32 	%r179, %r3, %r585;
	add.s32 	%r586, %r583, 6;
	mul.lo.s32 	%r180, %r3, %r586;
	mul.lo.s32 	%r181, %r87, %r1129;
	mov.b32 	%r1131, 0;
	bra.uni 	$L__BB0_162;
$L__BB0_173:
	ld.global.b8 	%r1136, [%rd59];
$L__BB0_174:
	shr.u32 	%r600, %r1136, %r171;
	and.b32 	%r601, %r600, %r172;
	cvt.rn.f32.u32 	%r602, %r601;
	add.rn.f32 	%r1130, %r184, %r602;
	add.s32 	%r1132, %r1132, -4;
	add.s32 	%r1131, %r1131, %r177;
	setp.eq.b32 	%p58, %r1132, 0;
	@%p58 bra 	$L__BB0_175;
$L__BB0_162:
	add.s32 	%r587, %r181, %r1131;
	cvt.u64.u32 	%rd108, %r587;
	add.s64 	%rd56, %rd54, %rd108;
	@%p122 bra 	$L__BB0_164;
	ld.global.b8 	%r1133, [%rd56];
	bra.uni 	$L__BB0_165;
$L__BB0_164:
	ld.global.b16 	%r1133, [%rd56];
$L__BB0_165:
	shr.u32 	%r588, %r1133, %r171;
	and.b32 	%r589, %r588, %r172;
	cvt.rn.f32.u32 	%r590, %r589;
	add.rn.f32 	%r182, %r1130, %r590;
	add.s32 	%r591, %r178, %r1131;
	cvt.u64.u32 	%rd109, %r591;
	add.s64 	%rd57, %rd54, %rd109;
	@%p122 bra 	$L__BB0_166;
	ld.global.b8 	%r1134, [%rd57];
	bra.uni 	$L__BB0_168;
$L__BB0_166:
	ld.global.b16 	%r1134, [%rd57];
$L__BB0_168:
	shr.u32 	%r592, %r1134, %r171;
	and.b32 	%r593, %r592, %r172;
	cvt.rn.f32.u32 	%r594, %r593;
	add.rn.f32 	%r183, %r182, %r594;
	add.s32 	%r595, %r179, %r1131;
	cvt.u64.u32 	%rd110, %r595;
	add.s64 	%rd58, %rd54, %rd110;
	@%p122 bra 	$L__BB0_169;
	ld.global.b8 	%r1135, [%rd58];
	bra.uni 	$L__BB0_171;
$L__BB0_169:
	ld.global.b16 	%r1135, [%rd58];
$L__BB0_171:
	shr.u32 	%r596, %r1135, %r171;
	and.b32 	%r597, %r596, %r172;
	cvt.rn.f32.u32 	%r598, %r597;
	add.rn.f32 	%r184, %r183, %r598;
	add.s32 	%r599, %r180, %r1131;
	cvt.u64.u32 	%rd111, %r599;
	add.s64 	%rd59, %rd54, %rd111;
	@!%p122 bra 	$L__BB0_173;
	ld.global.b16 	%r1136, [%rd59];
	bra.uni 	$L__BB0_174;
$L__BB0_176:
	cvt.rn.f32.u32 	%r538, %r537;
	cvt.rn.f32.u32 	%r169, %r578;
	div.rn.f32 	%r86, %r1088, %r412;
	div.rn.f32 	%r125, %r1100, %r506;
	div.rn.f32 	%r126, %r1099, %r506;
	ld.param.b32 	%r186, [vidcut_place_param_0+128];
	setp.eq.b32 	%p60, %r186, 0;
	@%p60 bra 	$L__BB0_178;
	ld.param.b32 	%r603, [vidcut_place_param_0+100];
	cvt.rn.f32.u32 	%r1138, %r603;
	add.rn.f32 	%r604, %r1138, 0f3F800000;
	mul.rn.f32 	%r1137, %r604, 0f3F000000;
	div.rn.f32 	%r1140, %r86, %r1138;
	sub.rn.f32 	%r605, %r125, %r1137;
	div.rn.f32 	%r1139, %r605, %r1138;
	bra.uni 	$L__BB0_179;
$L__BB0_13:
	shl.b64 	%rd114, %rd2, 5;
	add.s64 	%rd6, %rd3, %rd114;
	ld.param.b64 	%rd116, [%rd3+48];
	cvta.to.global.u64 	%rd8, %rd116;
	ld.param.b32 	%r21, [%rd6+192];
	setp.eq.b32 	%p108, %r21, 0;
	ld.param.b32 	%r22, [%rd6+188];
	mov.b32 	%r1080, 0f00000000;
	mov.b32 	%r1081, %r1080;
	@%p108 bra 	$L__BB0_48;
	ld.param.b32 	%r934, [vidcut_place_param_0+120];
	add.s32 	%r935, %r934, -1;
	mul.wide.u32 	%rd115, %r935, 76;
	add.s64 	%rd7, %rd63, %rd115;
	ld.param.b64 	%rd9, [%rd3+64];
	cvt.rn.f32.u32 	%r936, %r1;
	cvt.rn.f32.u32 	%r23, %r22;
	cvt.rn.f32.u32 	%r937, %r221;
	cvt.rn.f32.u32 	%r24, %r21;
	ld.param.b32 	%r25, [vidcut_place_param_0+140];
	mul.rn.f32 	%r26, %r25, %r936;
	ld.param.b32 	%r27, [vidcut_place_param_0+144];
	mul.rn.f32 	%r28, %r27, %r937;
	ld.param.b32 	%r29, [vidcut_place_param_0+164];
	ld.param.v2.b32 	{%r30, %r31}, [vidcut_place_param_0+168];
	ld.param.b32 	%r32, [vidcut_place_param_0+176];
	ld.param.v2.b32 	{%r33, %r34}, [vidcut_place_param_0+96];
	mov.b32 	%r1068, 0;
	mov.b32 	%r1081, 0f00000000;
	setp.eq.b32 	%p109, %r22, 0;
	setp.eq.b32 	%p113, %r2, 0;
	mov.b32 	%r1080, %r1081;
	bra.uni 	$L__BB0_15;
$L__BB0_47:
	add.s32 	%r1068, %r1068, 1;
	setp.eq.b32 	%p116, %r1068, %r21;
	@%p116 bra 	$L__BB0_48;
$L__BB0_15:
	@%p109 bra 	$L__BB0_47;
	cvt.rn.f32.u32 	%r938, %r1068;
	add.rn.f32 	%r939, %r938, 0f3F000000;
	div.rn.f32 	%r940, %r939, %r24;
	mul.rn.f32 	%r941, %r940, %r27;
	add.rn.f32 	%r35, %r28, %r941;
	ld.param.b32 	%r36, [%rd7+284];
	ld.param.b32 	%r942, [%rd7+288];
	mul.rn.f32 	%r37, %r942, %r35;
	mov.b32 	%r1069, 0;
	bra.uni 	$L__BB0_17;
$L__BB0_46:
	add.s32 	%r1069, %r1069, 1;
	setp.eq.b32 	%p115, %r1069, %r22;
	@%p115 bra 	$L__BB0_47;
$L__BB0_17:
	cvt.rn.f32.u32 	%r943, %r1069;
	add.rn.f32 	%r944, %r943, 0f3F000000;
	div.rn.f32 	%r945, %r944, %r23;
	mul.rn.f32 	%r946, %r25, %r945;
	add.rn.f32 	%r38, %r26, %r946;
	mul.rn.f32 	%r947, %r36, %r38;
	add.rn.f32 	%r948, %r37, %r947;
	add.rn.f32 	%r39, %r948, 0f3F800000;
	abs.f32 	%r949, %r39;
	shl.b32 	%r950, %r39, 1;
	setp.gt.u32 	%p110, %r950, -16777217;
	setp.lt.f32 	%p111, %r949, 0f2B8CBCCC;
	or.pred 	%p112, %p110, %p111;
	@%p112 bra 	$L__BB0_46;
	@%p121 bra 	$L__BB0_46;
	ld.param.b32 	%r951, [%rd7+260];
	mul.rn.f32 	%r952, %r38, %r951;
	ld.param.b32 	%r953, [%rd7+264];
	mul.rn.f32 	%r954, %r35, %r953;
	add.rn.f32 	%r955, %r952, %r954;
	ld.param.b32 	%r956, [%rd7+268];
	add.rn.f32 	%r957, %r956, %r955;
	div.rn.f32 	%r958, %r957, %r39;
	ld.param.b32 	%r959, [%rd7+272];
	mul.rn.f32 	%r960, %r38, %r959;
	ld.param.b32 	%r961, [%rd7+276];
	mul.rn.f32 	%r962, %r35, %r961;
	add.rn.f32 	%r963, %r960, %r962;
	ld.param.b32 	%r964, [%rd7+280];
	add.rn.f32 	%r965, %r964, %r963;
	div.rn.f32 	%r966, %r965, %r39;
	mul.rn.f32 	%r967, %r958, %r31;
	add.rn.f32 	%r968, %r29, %r967;
	ld.param.b32 	%r969, [%rd6+196];
	div.rn.f32 	%r970, %r968, %r969;
	add.rn.f32 	%r971, %r970, 0fBF000000;
	ld.param.v2.b32 	{%r972, %r973}, [%rd6+200];
	ld.param.v2.b32 	{%r974, %r975}, [%rd6+208];
	max.f32 	%r976, %r975, %r973;
	max.f32 	%r977, %r971, %r973;
	min.f32 	%r978, %r977, %r976;
	mul.rn.f32 	%r979, %r966, %r32;
	add.rn.f32 	%r980, %r30, %r979;
	div.rn.f32 	%r981, %r980, %r972;
	add.rn.f32 	%r982, %r981, 0fBF000000;
	ld.param.b32 	%r983, [%rd6+216];
	max.f32 	%r984, %r983, %r974;
	max.f32 	%r985, %r982, %r974;
	min.f32 	%r986, %r985, %r984;
	cvt.rmi.f32.f32 	%r40, %r978;
	cvt.rmi.f32.f32 	%r41, %r986;
	sub.rn.f32 	%r42, %r978, %r40;
	cvt.rzi.u32.f32 	%r987, %r41;
	add.s32 	%r988, %r987, 1;
	ld.param.b32 	%r989, [%rd6+184];
	add.s32 	%r990, %r989, -1;
	min.u32 	%r991, %r988, %r990;
	cvt.u64.u32 	%rd119, %r987;
	mad.lo.s64 	%rd120, %rd9, %rd119, %rd8;
	cvt.rzi.u32.f32 	%r992, %r40;
	add.s32 	%r993, %r992, 1;
	ld.param.b32 	%r994, [%rd6+180];
	add.s32 	%r995, %r994, -1;
	min.u32 	%r996, %r993, %r995;
	mul.lo.s32 	%r997, %r4, %r992;
	cvt.u64.u32 	%rd121, %r997;
	add.s64 	%rd10, %rd120, %rd121;
	mul.lo.s32 	%r999, %r996, %r4;
	cvt.u64.u32 	%rd122, %r999;
	@%p122 bra 	$L__BB0_20;
	ld.global.b8 	%r1070, [%rd10];
	bra.uni 	$L__BB0_22;
$L__BB0_20:
	ld.global.b16 	%r1070, [%rd10];
$L__BB0_22:
	cvt.u64.u32 	%rd117, %r991;
	mov.b32 	%r998, 0f3F800000;
	sub.rn.f32 	%r44, %r998, %r42;
	add.s64 	%rd11, %rd120, %rd122;
	shr.u32 	%r1000, %r1070, %r33;
	and.b32 	%r1001, %r1000, %r34;
	cvt.rn.f32.u32 	%r1002, %r1001;
	@%p122 bra 	$L__BB0_23;
	ld.global.b8 	%r1071, [%rd11];
	bra.uni 	$L__BB0_25;
$L__BB0_23:
	ld.global.b16 	%r1071, [%rd11];
$L__BB0_25:
	sub.rn.f32 	%r43, %r986, %r41;
	mad.lo.s64 	%rd118, %rd9, %rd117, %rd8;
	mul.rn.f32 	%r46, %r44, %r1002;
	add.s64 	%rd12, %rd118, %rd121;
	shr.u32 	%r1003, %r1071, %r33;
	and.b32 	%r1004, %r1003, %r34;
	cvt.rn.f32.u32 	%r1005, %r1004;
	mul.rn.f32 	%r1006, %r42, %r1005;
	@%p122 bra 	$L__BB0_26;
	ld.global.b8 	%r1072, [%rd12];
	bra.uni 	$L__BB0_28;
$L__BB0_26:
	ld.global.b16 	%r1072, [%rd12];
$L__BB0_28:
	add.s64 	%rd13, %rd118, %rd122;
	sub.rn.f32 	%r45, %r998, %r43;
	add.rn.f32 	%r47, %r46, %r1006;
	shr.u32 	%r1007, %r1072, %r33;
	and.b32 	%r1008, %r1007, %r34;
	cvt.rn.f32.u32 	%r1009, %r1008;
	mul.rn.f32 	%r48, %r44, %r1009;
	@%p122 bra 	$L__BB0_29;
	ld.global.b8 	%r1073, [%rd13];
	bra.uni 	$L__BB0_31;
$L__BB0_29:
	ld.global.b16 	%r1073, [%rd13];
$L__BB0_31:
	shr.u32 	%r1010, %r1073, %r33;
	and.b32 	%r1011, %r1010, %r34;
	cvt.rn.f32.u32 	%r1012, %r1011;
	mul.rn.f32 	%r1013, %r42, %r1012;
	add.rn.f32 	%r1014, %r48, %r1013;
	mul.rn.f32 	%r1015, %r45, %r47;
	mul.rn.f32 	%r1016, %r43, %r1014;
	add.rn.f32 	%r1017, %r1015, %r1016;
	add.rn.f32 	%r1081, %r1081, %r1017;
	@%p113 bra 	$L__BB0_46;
	mov.b32 	%r1074, %r3;
	mov.b32 	%r1075, %r2;
	bra.uni 	$L__BB0_33;
$L__BB0_44:
	ld.global.b16 	%r1079, [%rd18];
$L__BB0_45:
	cvt.u32.u64 	%r1028, %rd14;
	shr.u32 	%r1029, %r1079, %r33;
	and.b32 	%r1030, %r1029, %r34;
	cvt.rn.f32.u32 	%r1031, %r1030;
	mul.rn.f32 	%r1032, %r42, %r1031;
	add.rn.f32 	%r1033, %r51, %r1032;
	mul.rn.f32 	%r1034, %r45, %r50;
	mul.rn.f32 	%r1035, %r43, %r1033;
	add.rn.f32 	%r1036, %r1034, %r1035;
	add.rn.f32 	%r1080, %r1080, %r1036;
	add.s32 	%r1075, %r1075, -1;
	add.s32 	%r1074, %r1028, %r3;
	setp.ne.b32 	%p114, %r1075, 0;
	@!%p114 bra 	$L__BB0_46;
$L__BB0_33:
	cvt.u64.u32 	%rd14, %r1074;
	add.s64 	%rd15, %rd10, %rd14;
	@%p122 bra 	$L__BB0_35;
	ld.global.b8 	%r1076, [%rd15];
	bra.uni 	$L__BB0_36;
$L__BB0_35:
	ld.global.b16 	%r1076, [%rd15];
$L__BB0_36:
	shr.u32 	%r1018, %r1076, %r33;
	and.b32 	%r1019, %r1018, %r34;
	cvt.rn.f32.u32 	%r1020, %r1019;
	add.s64 	%rd16, %rd11, %rd14;
	@%p122 bra 	$L__BB0_38;
	ld.global.b8 	%r1077, [%rd16];
	bra.uni 	$L__BB0_39;
$L__BB0_38:
	ld.global.b16 	%r1077, [%rd16];
$L__BB0_39:
	mul.rn.f32 	%r49, %r44, %r1020;
	shr.u32 	%r1021, %r1077, %r33;
	and.b32 	%r1022, %r1021, %r34;
	cvt.rn.f32.u32 	%r1023, %r1022;
	mul.rn.f32 	%r1024, %r42, %r1023;
	add.s64 	%rd17, %rd12, %rd14;
	@%p122 bra 	$L__BB0_41;
	ld.global.b8 	%r1078, [%rd17];
	bra.uni 	$L__BB0_42;
$L__BB0_41:
	ld.global.b16 	%r1078, [%rd17];
$L__BB0_42:
	add.rn.f32 	%r50, %r49, %r1024;
	shr.u32 	%r1025, %r1078, %r33;
	and.b32 	%r1026, %r1025, %r34;
	cvt.rn.f32.u32 	%r1027, %r1026;
	mul.rn.f32 	%r51, %r44, %r1027;
	add.s64 	%rd18, %rd13, %rd14;
	@%p122 bra 	$L__BB0_44;
	ld.global.b8 	%r1079, [%rd18];
	bra.uni 	$L__BB0_45;
$L__BB0_48:
	mul.lo.s32 	%r1037, %r22, %r21;
	cvt.rn.f32.u32 	%r1038, %r1037;
	div.rn.f32 	%r1039, %r1081, %r1038;
	div.rn.f32 	%r1040, %r1080, %r1038;
	st.local.v2.b32 	[%rd125], {%r1039, %r1040};
	bra.uni 	$L__BB0_307;
$L__BB0_178:
	ld.param.b32 	%r606, [vidcut_place_param_0+160];
	mul.rn.f32 	%r607, %r606, 0fC1800000;
	add.rn.f32 	%r608, %r86, %r607;
	mul.rn.f32 	%r609, %r606, 0f435B0000;
	div.rn.f32 	%r1140, %r608, %r609;
	mul.rn.f32 	%r1137, %r606, 0f43000000;
	sub.rn.f32 	%r610, %r125, %r1137;
	mul.rn.f32 	%r1138, %r606, 0f43600000;
	div.rn.f32 	%r1139, %r610, %r1138;
$L__BB0_179:
	div.rn.f32 	%r151, %r1106, %r538;
	div.rn.f32 	%r170, %r1118, %r169;
	div.rn.f32 	%r185, %r1130, %r169;
	sub.rn.f32 	%r611, %r126, %r1137;
	div.rn.f32 	%r612, %r611, %r1138;
	ld.param.v2.b32 	{%r187, %r188}, [vidcut_place_param_0+152];
	mov.b32 	%r613, 0f3F800000;
	sub.rn.f32 	%r614, %r613, %r187;
	add.rn.f32 	%r189, %r614, %r614;
	mul.rn.f32 	%r615, %r612, %r189;
	add.rn.f32 	%r1148, %r1140, %r615;
	sub.rn.f32 	%r616, %r613, %r188;
	add.rn.f32 	%r190, %r616, %r616;
	mul.rn.f32 	%r617, %r1139, %r190;
	add.rn.f32 	%r1152, %r1140, %r617;
	mul.rn.f32 	%r618, %r187, %r1148;
	sub.rn.f32 	%r619, %r1140, %r618;
	mul.rn.f32 	%r620, %r188, %r1152;
	sub.rn.f32 	%r621, %r619, %r620;
	sub.rn.f32 	%r191, %r614, %r188;
	@%p60 bra 	$L__BB0_181;
	ld.param.b32 	%r622, [vidcut_place_param_0+100];
	cvt.rn.f32.u32 	%r1142, %r622;
	add.rn.f32 	%r623, %r1142, 0f3F800000;
	mul.rn.f32 	%r1141, %r623, 0f3F000000;
	div.rn.f32 	%r1144, %r151, %r1142;
	sub.rn.f32 	%r624, %r170, %r1141;
	div.rn.f32 	%r1143, %r624, %r1142;
	bra.uni 	$L__BB0_182;
$L__BB0_181:
	ld.param.b32 	%r625, [vidcut_place_param_0+160];
	mul.rn.f32 	%r626, %r625, 0fC1800000;
	add.rn.f32 	%r627, %r151, %r626;
	mul.rn.f32 	%r628, %r625, 0f435B0000;
	div.rn.f32 	%r1144, %r627, %r628;
	mul.rn.f32 	%r1141, %r625, 0f43000000;
	sub.rn.f32 	%r629, %r170, %r1141;
	mul.rn.f32 	%r1142, %r625, 0f43600000;
	div.rn.f32 	%r1143, %r629, %r1142;
$L__BB0_182:
	div.rn.f32 	%r1150, %r621, %r191;
	sub.rn.f32 	%r631, %r185, %r1141;
	div.rn.f32 	%r632, %r631, %r1142;
	mul.rn.f32 	%r633, %r189, %r632;
	add.rn.f32 	%r192, %r1144, %r633;
	mul.rn.f32 	%r634, %r190, %r1143;
	add.rn.f32 	%r193, %r1144, %r634;
	mul.rn.f32 	%r635, %r187, %r192;
	sub.rn.f32 	%r636, %r1144, %r635;
	mul.rn.f32 	%r637, %r188, %r193;
	sub.rn.f32 	%r638, %r636, %r637;
	div.rn.f32 	%r194, %r638, %r191;
	add.s32 	%r630, %r20, -1;
	setp.gt.u32 	%p61, %r630, 25;
	@%p61 bra 	$L__BB0_231;
	$L_brx_0: .branchtargets
		$L__BB0_197,
		$L__BB0_198,
		$L__BB0_199,
		$L__BB0_200,
		$L__BB0_201,
		$L__BB0_202,
		$L__BB0_205,
		$L__BB0_208,
		$L__BB0_214,
		$L__BB0_215,
		$L__BB0_216,
		$L__BB0_217,
		$L__BB0_218,
		$L__BB0_184,
		$L__BB0_187,
		$L__BB0_190,
		$L__BB0_190,
		$L__BB0_219,
		$L__BB0_197,
		$L__BB0_222,
		$L__BB0_223,
		$L__BB0_224,
		$L__BB0_227,
		$L__BB0_230,
		$L__BB0_193,
		$L__BB0_193;
	brx.idx 	%r630, $L_brx_0;
$L__BB0_197:
	add.rn.f32 	%r1148, %r1148, %r192;
	bra.uni 	$L__BB0_231;
$L__BB0_193:
	add.rn.f32 	%r698, %r1148, %r1150;
	add.rn.f32 	%r199, %r1152, %r698;
	add.rn.f32 	%r699, %r192, %r194;
	add.rn.f32 	%r200, %r193, %r699;
	setp.ne.b32 	%p75, %r20, 25;
	@%p75 bra 	$L__BB0_195;
	setp.gt.f32 	%p77, %r199, %r200;
	@%p77 bra 	$L__BB0_303;
	bra.uni 	$L__BB0_196;
$L__BB0_190:
	mul.rn.f32 	%r700, %r1148, 0f3E991687;
	mul.rn.f32 	%r701, %r1150, 0f3F1645A2;
	add.rn.f32 	%r702, %r700, %r701;
	mul.rn.f32 	%r703, %r1152, 0f3DE978D5;
	add.rn.f32 	%r197, %r703, %r702;
	mul.rn.f32 	%r704, %r192, 0f3E991687;
	mul.rn.f32 	%r705, %r194, 0f3F1645A2;
	add.rn.f32 	%r706, %r704, %r705;
	mul.rn.f32 	%r707, %r193, 0f3DE978D5;
	add.rn.f32 	%r198, %r707, %r706;
	setp.ne.b32 	%p78, %r20, 16;
	@%p78 bra 	$L__BB0_192;
	sub.rn.f32 	%r721, %r1148, %r197;
	div.rn.f32 	%r722, %r721, 0f3FB374BC;
	sub.rn.f32 	%r723, %r1152, %r197;
	div.rn.f32 	%r724, %r723, 0f3FE2D0E5;
	mul.rn.f32 	%r725, %r724, 0fBAAF1CB9;
	add.rn.f32 	%r726, %r198, %r725;
	mul.rn.f32 	%r727, %r722, 0f3FB3820A;
	add.rn.f32 	%r1148, %r727, %r726;
	mul.rn.f32 	%r728, %r724, 0fBEAFD977;
	add.rn.f32 	%r729, %r198, %r728;
	mul.rn.f32 	%r730, %r722, 0fBF36E6A7;
	add.rn.f32 	%r1150, %r729, %r730;
	mul.rn.f32 	%r731, %r724, 0f3FE2A2C2;
	add.rn.f32 	%r732, %r731, %r198;
	mul.rn.f32 	%r733, %r722, 0f3B03EC89;
	add.rn.f32 	%r1152, %r733, %r732;
	bra.uni 	$L__BB0_303;
$L__BB0_184:
	sub.rn.f32 	%r762, %r1148, %r1150;
	sub.rn.f32 	%r763, %r1150, %r1152;
	sub.rn.f32 	%r764, %r1152, %r1148;
	mul.rn.f32 	%r765, %r762, %r762;
	mul.rn.f32 	%r766, %r763, %r763;
	add.rn.f32 	%r767, %r765, %r766;
	mul.rn.f32 	%r768, %r764, %r764;
	add.rn.f32 	%r195, %r768, %r767;
	setp.leu.f32 	%p80, %r195, 0f00000000;
	mov.b32 	%r1145, 0f00000000;
	@%p80 bra 	$L__BB0_186;
	sqrt.rn.f32 	%r769, %r195;
	sub.rn.f32 	%r770, %r192, %r194;
	sub.rn.f32 	%r771, %r194, %r193;
	sub.rn.f32 	%r772, %r193, %r192;
	mul.rn.f32 	%r773, %r770, %r770;
	mul.rn.f32 	%r774, %r771, %r771;
	add.rn.f32 	%r775, %r773, %r774;
	mul.rn.f32 	%r776, %r772, %r772;
	add.rn.f32 	%r777, %r776, %r775;
	sqrt.rn.f32 	%r778, %r777;
	div.rn.f32 	%r1145, %r778, %r769;
$L__BB0_186:
	mul.rn.f32 	%r779, %r1148, 0f3E59B3D0;
	mul.rn.f32 	%r780, %r1150, 0f3F371759;
	add.rn.f32 	%r781, %r779, %r780;
	mul.rn.f32 	%r782, %r1152, 0f3D93DD98;
	add.rn.f32 	%r783, %r782, %r781;
	mul.rn.f32 	%r784, %r192, 0f3E59B3D0;
	mul.rn.f32 	%r785, %r194, 0f3F371759;
	add.rn.f32 	%r786, %r784, %r785;
	mul.rn.f32 	%r787, %r193, 0f3D93DD98;
	add.rn.f32 	%r788, %r787, %r786;
	sub.rn.f32 	%r789, %r1148, %r783;
	mul.rn.f32 	%r790, %r789, %r1145;
	add.rn.f32 	%r1148, %r788, %r790;
	sub.rn.f32 	%r791, %r1150, %r783;
	mul.rn.f32 	%r792, %r791, %r1145;
	add.rn.f32 	%r1150, %r788, %r792;
	sub.rn.f32 	%r793, %r1152, %r783;
	mul.rn.f32 	%r794, %r793, %r1145;
	add.rn.f32 	%r1152, %r788, %r794;
	bra.uni 	$L__BB0_303;
$L__BB0_216:
	setp.ltu.f32 	%p70, %r1148, 0f3F800000;
	mov.b32 	%r663, 0f3F800000;
	sub.rn.f32 	%r664, %r663, %r1148;
	div.rn.f32 	%r665, %r192, %r664;
	min.f32 	%r666, %r665, 0f3F800000;
	selp.f32 	%r1148, %r666, 0f3F800000, %p70;
	bra.uni 	$L__BB0_231;
$L__BB0_200:
	mul.rn.f32 	%r1148, %r1148, %r192;
	bra.uni 	$L__BB0_231;
$L__BB0_217:
	setp.gtu.f32 	%p69, %r1148, 0f00000000;
	mov.b32 	%r658, 0f3F800000;
	sub.rn.f32 	%r659, %r658, %r192;
	div.rn.f32 	%r660, %r659, %r1148;
	min.f32 	%r661, %r660, 0f3F800000;
	sub.rn.f32 	%r662, %r658, %r661;
	selp.f32 	%r1148, %r662, 0f00000000, %p69;
	bra.uni 	$L__BB0_231;
$L__BB0_214:
	min.f32 	%r1148, %r1148, %r192;
	bra.uni 	$L__BB0_231;
$L__BB0_198:
	sub.rn.f32 	%r1148, %r192, %r1148;
	bra.uni 	$L__BB0_231;
$L__BB0_195:
	setp.lt.f32 	%p76, %r199, %r200;
	@%p76 bra 	$L__BB0_303;
$L__BB0_196:
	mov.b32 	%r1152, %r193;
	mov.b32 	%r1150, %r194;
	mov.b32 	%r1148, %r192;
	bra.uni 	$L__BB0_303;
$L__BB0_187:
	sub.rn.f32 	%r734, %r192, %r194;
	sub.rn.f32 	%r735, %r194, %r193;
	sub.rn.f32 	%r736, %r193, %r192;
	mul.rn.f32 	%r737, %r734, %r734;
	mul.rn.f32 	%r738, %r735, %r735;
	add.rn.f32 	%r739, %r737, %r738;
	mul.rn.f32 	%r740, %r736, %r736;
	add.rn.f32 	%r196, %r740, %r739;
	setp.leu.f32 	%p79, %r196, 0f00000000;
	mov.b32 	%r1146, 0f00000000;
	@%p79 bra 	$L__BB0_189;
	sqrt.rn.f32 	%r741, %r196;
	sub.rn.f32 	%r742, %r1148, %r1150;
	sub.rn.f32 	%r743, %r1150, %r1152;
	sub.rn.f32 	%r744, %r1152, %r1148;
	mul.rn.f32 	%r745, %r742, %r742;
	mul.rn.f32 	%r746, %r743, %r743;
	add.rn.f32 	%r747, %r745, %r746;
	mul.rn.f32 	%r748, %r744, %r744;
	add.rn.f32 	%r749, %r748, %r747;
	sqrt.rn.f32 	%r750, %r749;
	div.rn.f32 	%r1146, %r750, %r741;
$L__BB0_189:
	mul.rn.f32 	%r751, %r192, 0f3E59B3D0;
	mul.rn.f32 	%r752, %r194, 0f3F371759;
	add.rn.f32 	%r753, %r751, %r752;
	mul.rn.f32 	%r754, %r193, 0f3D93DD98;
	add.rn.f32 	%r755, %r754, %r753;
	sub.rn.f32 	%r756, %r192, %r755;
	mul.rn.f32 	%r757, %r756, %r1146;
	add.rn.f32 	%r1148, %r755, %r757;
	sub.rn.f32 	%r758, %r194, %r755;
	mul.rn.f32 	%r759, %r758, %r1146;
	add.rn.f32 	%r1150, %r755, %r759;
	sub.rn.f32 	%r760, %r193, %r755;
	mul.rn.f32 	%r761, %r760, %r1146;
	add.rn.f32 	%r1152, %r755, %r761;
	bra.uni 	$L__BB0_303;
$L__BB0_199:
	sub.rn.f32 	%r697, %r1148, %r192;
	abs.f32 	%r1148, %r697;
	bra.uni 	$L__BB0_231;
$L__BB0_205:
	setp.gtu.f32 	%p73, %r1148, 0f3F000000;
	@!%p73 bra 	$L__BB0_206;
	mov.b32 	%r682, 0f3F800000;
	sub.rn.f32 	%r683, %r682, %r1148;
	add.rn.f32 	%r684, %r683, %r683;
	sub.rn.f32 	%r685, %r682, %r192;
	mul.rn.f32 	%r686, %r684, %r685;
	sub.rn.f32 	%r1148, %r682, %r686;
	bra.uni 	$L__BB0_231;
$L__BB0_192:
	sub.rn.f32 	%r708, %r192, %r198;
	div.rn.f32 	%r709, %r708, 0f3FB374BC;
	sub.rn.f32 	%r710, %r193, %r198;
	div.rn.f32 	%r711, %r710, 0f3FE2D0E5;
	mul.rn.f32 	%r712, %r711, 0fBAAF1CB9;
	add.rn.f32 	%r713, %r197, %r712;
	mul.rn.f32 	%r714, %r709, 0f3FB3820A;
	add.rn.f32 	%r1148, %r714, %r713;
	mul.rn.f32 	%r715, %r711, 0fBEAFD977;
	add.rn.f32 	%r716, %r197, %r715;
	mul.rn.f32 	%r717, %r709, 0fBF36E6A7;
	add.rn.f32 	%r1150, %r716, %r717;
	mul.rn.f32 	%r718, %r711, 0f3FE2A2C2;
	add.rn.f32 	%r719, %r197, %r718;
	mul.rn.f32 	%r720, %r709, 0f3B03EC89;
	add.rn.f32 	%r1152, %r720, %r719;
	bra.uni 	$L__BB0_303;
$L__BB0_201:
	mov.b32 	%r693, 0f3F800000;
	sub.rn.f32 	%r694, %r693, %r1148;
	sub.rn.f32 	%r695, %r693, %r192;
	mul.rn.f32 	%r696, %r694, %r695;
	sub.rn.f32 	%r1148, %r693, %r696;
	bra.uni 	$L__BB0_231;
$L__BB0_218:
	add.rn.f32 	%r655, %r1148, %r192;
	add.rn.f32 	%r656, %r1148, %r1148;
	mul.rn.f32 	%r657, %r656, %r192;
	sub.rn.f32 	%r1148, %r655, %r657;
	bra.uni 	$L__BB0_231;
$L__BB0_202:
	setp.gtu.f32 	%p74, %r192, 0f3F000000;
	@!%p74 bra 	$L__BB0_203;
	mov.b32 	%r688, 0f3F800000;
	sub.rn.f32 	%r689, %r688, %r192;
	add.rn.f32 	%r690, %r689, %r689;
	sub.rn.f32 	%r691, %r688, %r1148;
	mul.rn.f32 	%r692, %r691, %r690;
	sub.rn.f32 	%r1148, %r688, %r692;
	bra.uni 	$L__BB0_231;
$L__BB0_215:
	max.f32 	%r1148, %r1148, %r192;
	bra.uni 	$L__BB0_231;
$L__BB0_208:
	setp.gtu.f32 	%p71, %r1148, 0f3F000000;
	@!%p71 bra 	$L__BB0_209;
	setp.gtu.f32 	%p72, %r192, 0f3E800000;
	@!%p72 bra 	$L__BB0_211;
	sqrt.rn.f32 	%r1147, %r192;
	bra.uni 	$L__BB0_213;
$L__BB0_223:
	add.rn.f32 	%r651, %r1148, %r1148;
	add.rn.f32 	%r652, %r651, %r192;
	add.rn.f32 	%r1148, %r652, 0fBF800000;
	bra.uni 	$L__BB0_231;
$L__BB0_224:
	setp.gtu.f32 	%p64, %r1148, 0f3F000000;
	add.rn.f32 	%r201, %r1148, %r1148;
	@!%p64 bra 	$L__BB0_225;
	add.rn.f32 	%r646, %r201, 0fBF800000;
	setp.ltu.f32 	%p66, %r646, 0f3F800000;
	mov.b32 	%r647, 0f3F800000;
	sub.rn.f32 	%r648, %r647, %r646;
	div.rn.f32 	%r649, %r192, %r648;
	min.f32 	%r650, %r649, 0f3F800000;
	selp.f32 	%r1148, %r650, 0f3F800000, %p66;
	bra.uni 	$L__BB0_231;
$L__BB0_219:
	setp.gtu.f32 	%p67, %r1148, 0f00000000;
	@!%p67 bra 	$L__BB0_220;
	div.rn.f32 	%r654, %r192, %r1148;
	min.f32 	%r1148, %r654, 0f3F800000;
	bra.uni 	$L__BB0_231;
$L__BB0_227:
	setp.gtu.f32 	%p63, %r1148, 0f3F000000;
	add.rn.f32 	%r202, %r1148, %r1148;
	@!%p63 bra 	$L__BB0_228;
	add.rn.f32 	%r640, %r202, 0fBF800000;
	max.f32 	%r1148, %r192, %r640;
	bra.uni 	$L__BB0_231;
$L__BB0_222:
	add.rn.f32 	%r653, %r1148, %r192;
	add.rn.f32 	%r1148, %r653, 0fBF800000;
	bra.uni 	$L__BB0_231;
$L__BB0_230:
	add.rn.f32 	%r639, %r1148, %r192;
	setp.ge.f32 	%p62, %r639, 0f3F800000;
	selp.f32 	%r1148, 0f3F800000, 0f00000000, %p62;
	bra.uni 	$L__BB0_231;
$L__BB0_206:
	add.rn.f32 	%r681, %r1148, %r1148;
	mul.rn.f32 	%r1148, %r681, %r192;
	bra.uni 	$L__BB0_231;
$L__BB0_203:
	add.rn.f32 	%r687, %r192, %r192;
	mul.rn.f32 	%r1148, %r1148, %r687;
	bra.uni 	$L__BB0_231;
$L__BB0_209:
	add.rn.f32 	%r667, %r1148, %r1148;
	mov.b32 	%r668, 0f3F800000;
	sub.rn.f32 	%r669, %r668, %r667;
	mul.rn.f32 	%r670, %r669, %r192;
	sub.rn.f32 	%r671, %r668, %r192;
	mul.rn.f32 	%r672, %r671, %r670;
	sub.rn.f32 	%r1148, %r192, %r672;
	bra.uni 	$L__BB0_231;
$L__BB0_225:
	setp.gtu.f32 	%p65, %r201, 0f00000000;
	mov.b32 	%r641, 0f3F800000;
	sub.rn.f32 	%r642, %r641, %r192;
	div.rn.f32 	%r643, %r642, %r201;
	min.f32 	%r644, %r643, 0f3F800000;
	sub.rn.f32 	%r645, %r641, %r644;
	selp.f32 	%r1148, %r645, 0f00000000, %p65;
	bra.uni 	$L__BB0_231;
$L__BB0_220:
	setp.gt.f32 	%p68, %r192, 0f00000000;
	selp.f32 	%r1148, 0f3F800000, 0f00000000, %p68;
	bra.uni 	$L__BB0_231;
$L__BB0_228:
	min.f32 	%r1148, %r192, %r202;
	bra.uni 	$L__BB0_231;
$L__BB0_211:
	mul.rn.f32 	%r673, %r192, 0f41800000;
	add.rn.f32 	%r674, %r673, 0fC1400000;
	mul.rn.f32 	%r675, %r192, %r674;
	add.rn.f32 	%r676, %r675, 0f40800000;
	mul.rn.f32 	%r1147, %r192, %r676;
$L__BB0_213:
	add.rn.f32 	%r677, %r1148, %r1148;
	add.rn.f32 	%r678, %r677, 0fBF800000;
	sub.rn.f32 	%r679, %r1147, %r192;
	mul.rn.f32 	%r680, %r678, %r679;
	add.rn.f32 	%r1148, %r192, %r680;
$L__BB0_231:
	setp.gt.u32 	%p81, %r630, 23;
	@%p81 bra 	$L__BB0_267;
	$L_brx_1: .branchtargets
		$L__BB0_266,
		$L__BB0_265,
		$L__BB0_264,
		$L__BB0_263,
		$L__BB0_262,
		$L__BB0_259,
		$L__BB0_256,
		$L__BB0_250,
		$L__BB0_249,
		$L__BB0_248,
		$L__BB0_247,
		$L__BB0_246,
		$L__BB0_245,
		$L__BB0_267,
		$L__BB0_267,
		$L__BB0_267,
		$L__BB0_267,
		$L__BB0_242,
		$L__BB0_266,
		$L__BB0_241,
		$L__BB0_240,
		$L__BB0_237,
		$L__BB0_234,
		$L__BB0_233;
	brx.idx 	%r630, $L_brx_1;
$L__BB0_266:
	add.rn.f32 	%r1150, %r1150, %r194;
	bra.uni 	$L__BB0_267;
$L__BB0_234:
	setp.gtu.f32 	%p83, %r1150, 0f3F000000;
	add.rn.f32 	%r203, %r1150, %r1150;
	@!%p83 bra 	$L__BB0_235;
	add.rn.f32 	%r796, %r203, 0fBF800000;
	max.f32 	%r1150, %r194, %r796;
	bra.uni 	$L__BB0_267;
$L__BB0_245:
	add.rn.f32 	%r811, %r1150, %r194;
	add.rn.f32 	%r812, %r1150, %r1150;
	mul.rn.f32 	%r813, %r812, %r194;
	sub.rn.f32 	%r1150, %r811, %r813;
	bra.uni 	$L__BB0_267;
$L__BB0_248:
	max.f32 	%r1150, %r1150, %r194;
	bra.uni 	$L__BB0_267;
$L__BB0_246:
	setp.gtu.f32 	%p89, %r1150, 0f00000000;
	mov.b32 	%r814, 0f3F800000;
	sub.rn.f32 	%r815, %r814, %r194;
	div.rn.f32 	%r816, %r815, %r1150;
	min.f32 	%r817, %r816, 0f3F800000;
	sub.rn.f32 	%r818, %r814, %r817;
	selp.f32 	%r1150, %r818, 0f00000000, %p89;
	bra.uni 	$L__BB0_267;
$L__BB0_233:
	add.rn.f32 	%r795, %r1150, %r194;
	setp.ge.f32 	%p82, %r795, 0f3F800000;
	selp.f32 	%r1150, 0f3F800000, 0f00000000, %p82;
	bra.uni 	$L__BB0_267;
$L__BB0_265:
	sub.rn.f32 	%r1150, %r194, %r1150;
	bra.uni 	$L__BB0_267;
$L__BB0_262:
	mov.b32 	%r849, 0f3F800000;
	sub.rn.f32 	%r850, %r849, %r1150;
	sub.rn.f32 	%r851, %r849, %r194;
	mul.rn.f32 	%r852, %r850, %r851;
	sub.rn.f32 	%r1150, %r849, %r852;
	bra.uni 	$L__BB0_267;
$L__BB0_247:
	setp.ltu.f32 	%p90, %r1150, 0f3F800000;
	mov.b32 	%r819, 0f3F800000;
	sub.rn.f32 	%r820, %r819, %r1150;
	div.rn.f32 	%r821, %r194, %r820;
	min.f32 	%r822, %r821, 0f3F800000;
	selp.f32 	%r1150, %r822, 0f3F800000, %p90;
	bra.uni 	$L__BB0_267;
$L__BB0_259:
	setp.gtu.f32 	%p94, %r194, 0f3F000000;
	@!%p94 bra 	$L__BB0_260;
	mov.b32 	%r844, 0f3F800000;
	sub.rn.f32 	%r845, %r844, %r194;
	add.rn.f32 	%r846, %r845, %r845;
	sub.rn.f32 	%r847, %r844, %r1150;
	mul.rn.f32 	%r848, %r847, %r846;
	sub.rn.f32 	%r1150, %r844, %r848;
	bra.uni 	$L__BB0_267;
$L__BB0_256:
	setp.gtu.f32 	%p93, %r1150, 0f3F000000;
	@!%p93 bra 	$L__BB0_257;
	mov.b32 	%r838, 0f3F800000;
	sub.rn.f32 	%r839, %r838, %r1150;
	add.rn.f32 	%r840, %r839, %r839;
	sub.rn.f32 	%r841, %r838, %r194;
	mul.rn.f32 	%r842, %r840, %r841;
	sub.rn.f32 	%r1150, %r838, %r842;
	bra.uni 	$L__BB0_267;
$L__BB0_242:
	setp.gtu.f32 	%p87, %r1150, 0f00000000;
	@!%p87 bra 	$L__BB0_243;
	div.rn.f32 	%r810, %r194, %r1150;
	min.f32 	%r1150, %r810, 0f3F800000;
	bra.uni 	$L__BB0_267;
$L__BB0_241:
	add.rn.f32 	%r809, %r1150, %r194;
	add.rn.f32 	%r1150, %r809, 0fBF800000;
	bra.uni 	$L__BB0_267;
$L__BB0_264:
	sub.rn.f32 	%r853, %r1150, %r194;
	abs.f32 	%r1150, %r853;
	bra.uni 	$L__BB0_267;
$L__BB0_263:
	mul.rn.f32 	%r1150, %r1150, %r194;
	bra.uni 	$L__BB0_267;
$L__BB0_249:
	min.f32 	%r1150, %r1150, %r194;
	bra.uni 	$L__BB0_267;
$L__BB0_250:
	setp.gtu.f32 	%p91, %r1150, 0f3F000000;
	@!%p91 bra 	$L__BB0_251;
	setp.gtu.f32 	%p92, %r194, 0f3E800000;
	@!%p92 bra 	$L__BB0_253;
	sqrt.rn.f32 	%r1149, %r194;
	bra.uni 	$L__BB0_255;
$L__BB0_237:
	setp.gtu.f32 	%p84, %r1150, 0f3F000000;
	add.rn.f32 	%r204, %r1150, %r1150;
	@!%p84 bra 	$L__BB0_238;
	add.rn.f32 	%r802, %r204, 0fBF800000;
	setp.ltu.f32 	%p86, %r802, 0f3F800000;
	mov.b32 	%r803, 0f3F800000;
	sub.rn.f32 	%r804, %r803, %r802;
	div.rn.f32 	%r805, %r194, %r804;
	min.f32 	%r806, %r805, 0f3F800000;
	selp.f32 	%r1150, %r806, 0f3F800000, %p86;
	bra.uni 	$L__BB0_267;
$L__BB0_240:
	add.rn.f32 	%r807, %r1150, %r1150;
	add.rn.f32 	%r808, %r807, %r194;
	add.rn.f32 	%r1150, %r808, 0fBF800000;
	bra.uni 	$L__BB0_267;
$L__BB0_235:
	min.f32 	%r1150, %r194, %r203;
	bra.uni 	$L__BB0_267;
$L__BB0_260:
	add.rn.f32 	%r843, %r194, %r194;
	mul.rn.f32 	%r1150, %r1150, %r843;
	bra.uni 	$L__BB0_267;
$L__BB0_257:
	add.rn.f32 	%r837, %r1150, %r1150;
	mul.rn.f32 	%r1150, %r837, %r194;
	bra.uni 	$L__BB0_267;
$L__BB0_243:
	setp.gt.f32 	%p88, %r194, 0f00000000;
	selp.f32 	%r1150, 0f3F800000, 0f00000000, %p88;
	bra.uni 	$L__BB0_267;
$L__BB0_251:
	add.rn.f32 	%r823, %r1150, %r1150;
	mov.b32 	%r824, 0f3F800000;
	sub.rn.f32 	%r825, %r824, %r823;
	mul.rn.f32 	%r826, %r825, %r194;
	sub.rn.f32 	%r827, %r824, %r194;
	mul.rn.f32 	%r828, %r827, %r826;
	sub.rn.f32 	%r1150, %r194, %r828;
	bra.uni 	$L__BB0_267;
$L__BB0_238:
	setp.gtu.f32 	%p85, %r204, 0f00000000;
	mov.b32 	%r797, 0f3F800000;
	sub.rn.f32 	%r798, %r797, %r194;
	div.rn.f32 	%r799, %r798, %r204;
	min.f32 	%r800, %r799, 0f3F800000;
	sub.rn.f32 	%r801, %r797, %r800;
	selp.f32 	%r1150, %r801, 0f00000000, %p85;
	bra.uni 	$L__BB0_267;
$L__BB0_253:
	mul.rn.f32 	%r829, %r194, 0f41800000;
	add.rn.f32 	%r830, %r829, 0fC1400000;
	mul.rn.f32 	%r831, %r194, %r830;
	add.rn.f32 	%r832, %r831, 0f40800000;
	mul.rn.f32 	%r1149, %r194, %r832;
$L__BB0_255:
	add.rn.f32 	%r833, %r1150, %r1150;
	add.rn.f32 	%r834, %r833, 0fBF800000;
	sub.rn.f32 	%r835, %r1149, %r194;
	mul.rn.f32 	%r836, %r834, %r835;
	add.rn.f32 	%r1150, %r194, %r836;
$L__BB0_267:
	@%p81 bra 	$L__BB0_303;
	$L_brx_2: .branchtargets
		$L__BB0_302,
		$L__BB0_301,
		$L__BB0_300,
		$L__BB0_299,
		$L__BB0_298,
		$L__BB0_295,
		$L__BB0_292,
		$L__BB0_286,
		$L__BB0_285,
		$L__BB0_284,
		$L__BB0_283,
		$L__BB0_282,
		$L__BB0_281,
		$L__BB0_303,
		$L__BB0_303,
		$L__BB0_303,
		$L__BB0_303,
		$L__BB0_278,
		$L__BB0_302,
		$L__BB0_277,
		$L__BB0_276,
		$L__BB0_273,
		$L__BB0_270,
		$L__BB0_269;
	brx.idx 	%r630, $L_brx_2;
$L__BB0_302:
	add.rn.f32 	%r1152, %r1152, %r193;
	bra.uni 	$L__BB0_303;
$L__BB0_270:
	setp.gtu.f32 	%p96, %r1152, 0f3F000000;
	add.rn.f32 	%r205, %r1152, %r1152;
	@!%p96 bra 	$L__BB0_271;
	add.rn.f32 	%r855, %r205, 0fBF800000;
	max.f32 	%r1152, %r193, %r855;
	bra.uni 	$L__BB0_303;
$L__BB0_281:
	add.rn.f32 	%r870, %r1152, %r193;
	add.rn.f32 	%r871, %r1152, %r1152;
	mul.rn.f32 	%r872, %r871, %r193;
	sub.rn.f32 	%r1152, %r870, %r872;
	bra.uni 	$L__BB0_303;
$L__BB0_284:
	max.f32 	%r1152, %r1152, %r193;
	bra.uni 	$L__BB0_303;
$L__BB0_282:
	setp.gtu.f32 	%p102, %r1152, 0f00000000;
	mov.b32 	%r873, 0f3F800000;
	sub.rn.f32 	%r874, %r873, %r193;
	div.rn.f32 	%r875, %r874, %r1152;
	min.f32 	%r876, %r875, 0f3F800000;
	sub.rn.f32 	%r877, %r873, %r876;
	selp.f32 	%r1152, %r877, 0f00000000, %p102;
	bra.uni 	$L__BB0_303;
$L__BB0_269:
	add.rn.f32 	%r854, %r1152, %r193;
	setp.ge.f32 	%p95, %r854, 0f3F800000;
	selp.f32 	%r1152, 0f3F800000, 0f00000000, %p95;
	bra.uni 	$L__BB0_303;
$L__BB0_301:
	sub.rn.f32 	%r1152, %r193, %r1152;
	bra.uni 	$L__BB0_303;
$L__BB0_298:
	mov.b32 	%r908, 0f3F800000;
	sub.rn.f32 	%r909, %r908, %r1152;
	sub.rn.f32 	%r910, %r908, %r193;
	mul.rn.f32 	%r911, %r909, %r910;
	sub.rn.f32 	%r1152, %r908, %r911;
	bra.uni 	$L__BB0_303;
$L__BB0_283:
	setp.ltu.f32 	%p103, %r1152, 0f3F800000;
	mov.b32 	%r878, 0f3F800000;
	sub.rn.f32 	%r879, %r878, %r1152;
	div.rn.f32 	%r880, %r193, %r879;
	min.f32 	%r881, %r880, 0f3F800000;
	selp.f32 	%r1152, %r881, 0f3F800000, %p103;
	bra.uni 	$L__BB0_303;
$L__BB0_295:
	setp.gtu.f32 	%p107, %r193, 0f3F000000;
	@!%p107 bra 	$L__BB0_296;
	mov.b32 	%r903, 0f3F800000;
	sub.rn.f32 	%r904, %r903, %r193;
	add.rn.f32 	%r905, %r904, %r904;
	sub.rn.f32 	%r906, %r903, %r1152;
	mul.rn.f32 	%r907, %r906, %r905;
	sub.rn.f32 	%r1152, %r903, %r907;
	bra.uni 	$L__BB0_303;
$L__BB0_292:
	setp.gtu.f32 	%p106, %r1152, 0f3F000000;
	@!%p106 bra 	$L__BB0_293;
	mov.b32 	%r897, 0f3F800000;
	sub.rn.f32 	%r898, %r897, %r1152;
	add.rn.f32 	%r899, %r898, %r898;
	sub.rn.f32 	%r900, %r897, %r193;
	mul.rn.f32 	%r901, %r899, %r900;
	sub.rn.f32 	%r1152, %r897, %r901;
	bra.uni 	$L__BB0_303;
$L__BB0_278:
	setp.gtu.f32 	%p100, %r1152, 0f00000000;
	@!%p100 bra 	$L__BB0_279;
	div.rn.f32 	%r869, %r193, %r1152;
	min.f32 	%r1152, %r869, 0f3F800000;
	bra.uni 	$L__BB0_303;
$L__BB0_277:
	add.rn.f32 	%r868, %r1152, %r193;
	add.rn.f32 	%r1152, %r868, 0fBF800000;
	bra.uni 	$L__BB0_303;
$L__BB0_300:
	sub.rn.f32 	%r912, %r1152, %r193;
	abs.f32 	%r1152, %r912;
	bra.uni 	$L__BB0_303;
$L__BB0_299:
	mul.rn.f32 	%r1152, %r1152, %r193;
	bra.uni 	$L__BB0_303;
$L__BB0_285:
	min.f32 	%r1152, %r1152, %r193;
	bra.uni 	$L__BB0_303;
$L__BB0_286:
	setp.gtu.f32 	%p104, %r1152, 0f3F000000;
	@!%p104 bra 	$L__BB0_287;
	setp.gtu.f32 	%p105, %r193, 0f3E800000;
	@!%p105 bra 	$L__BB0_289;
	sqrt.rn.f32 	%r1151, %r193;
	bra.uni 	$L__BB0_291;
$L__BB0_273:
	setp.gtu.f32 	%p97, %r1152, 0f3F000000;
	add.rn.f32 	%r206, %r1152, %r1152;
	@!%p97 bra 	$L__BB0_274;
	add.rn.f32 	%r861, %r206, 0fBF800000;
	setp.ltu.f32 	%p99, %r861, 0f3F800000;
	mov.b32 	%r862, 0f3F800000;
	sub.rn.f32 	%r863, %r862, %r861;
	div.rn.f32 	%r864, %r193, %r863;
	min.f32 	%r865, %r864, 0f3F800000;
	selp.f32 	%r1152, %r865, 0f3F800000, %p99;
	bra.uni 	$L__BB0_303;
$L__BB0_276:
	add.rn.f32 	%r866, %r1152, %r1152;
	add.rn.f32 	%r867, %r866, %r193;
	add.rn.f32 	%r1152, %r867, 0fBF800000;
	bra.uni 	$L__BB0_303;
$L__BB0_271:
	min.f32 	%r1152, %r193, %r205;
	bra.uni 	$L__BB0_303;
$L__BB0_296:
	add.rn.f32 	%r902, %r193, %r193;
	mul.rn.f32 	%r1152, %r1152, %r902;
	bra.uni 	$L__BB0_303;
$L__BB0_293:
	add.rn.f32 	%r896, %r1152, %r1152;
	mul.rn.f32 	%r1152, %r896, %r193;
	bra.uni 	$L__BB0_303;
$L__BB0_279:
	setp.gt.f32 	%p101, %r193, 0f00000000;
	selp.f32 	%r1152, 0f3F800000, 0f00000000, %p101;
	bra.uni 	$L__BB0_303;
$L__BB0_287:
	add.rn.f32 	%r882, %r1152, %r1152;
	mov.b32 	%r883, 0f3F800000;
	sub.rn.f32 	%r884, %r883, %r882;
	mul.rn.f32 	%r885, %r884, %r193;
	sub.rn.f32 	%r886, %r883, %r193;
	mul.rn.f32 	%r887, %r886, %r885;
	sub.rn.f32 	%r1152, %r193, %r887;
	bra.uni 	$L__BB0_303;
$L__BB0_274:
	setp.gtu.f32 	%p98, %r206, 0f00000000;
	mov.b32 	%r856, 0f3F800000;
	sub.rn.f32 	%r857, %r856, %r193;
	div.rn.f32 	%r858, %r857, %r206;
	min.f32 	%r859, %r858, 0f3F800000;
	sub.rn.f32 	%r860, %r856, %r859;
	selp.f32 	%r1152, %r860, 0f00000000, %p98;
	bra.uni 	$L__BB0_303;
$L__BB0_289:
	mul.rn.f32 	%r888, %r193, 0f41800000;
	add.rn.f32 	%r889, %r888, 0fC1400000;
	mul.rn.f32 	%r890, %r193, %r889;
	add.rn.f32 	%r891, %r890, 0f40800000;
	mul.rn.f32 	%r1151, %r193, %r891;
$L__BB0_291:
	add.rn.f32 	%r892, %r1152, %r1152;
	add.rn.f32 	%r893, %r892, 0fBF800000;
	sub.rn.f32 	%r894, %r1151, %r193;
	mul.rn.f32 	%r895, %r893, %r894;
	add.rn.f32 	%r1152, %r193, %r895;
$L__BB0_303:
	mul.rn.f32 	%r913, %r187, %r1148;
	mul.rn.f32 	%r914, %r191, %r1150;
	add.rn.f32 	%r915, %r914, %r913;
	mul.rn.f32 	%r916, %r188, %r1152;
	add.rn.f32 	%r207, %r916, %r915;
	sub.rn.f32 	%r917, %r1152, %r207;
	div.rn.f32 	%r208, %r917, %r190;
	sub.rn.f32 	%r918, %r1148, %r207;
	div.rn.f32 	%r209, %r918, %r189;
	@%p60 bra 	$L__BB0_305;
	ld.param.b32 	%r919, [vidcut_place_param_0+100];
	cvt.rn.f32.u32 	%r920, %r919;
	add.rn.f32 	%r921, %r920, 0f3F800000;
	mul.rn.f32 	%r922, %r921, 0f3F000000;
	mul.rn.f32 	%r1153, %r207, %r920;
	mul.rn.f32 	%r923, %r208, %r920;
	add.rn.f32 	%r1154, %r923, %r922;
	mul.rn.f32 	%r924, %r209, %r920;
	add.rn.f32 	%r1155, %r924, %r922;
	bra.uni 	$L__BB0_306;
$L__BB0_305:
	ld.param.b32 	%r925, [vidcut_place_param_0+160];
	mul.rn.f32 	%r926, %r925, 0f41800000;
	mul.rn.f32 	%r927, %r925, 0f435B0000;
	mul.rn.f32 	%r928, %r207, %r927;
	add.rn.f32 	%r1153, %r926, %r928;
	mul.rn.f32 	%r929, %r925, 0f43000000;
	mul.rn.f32 	%r930, %r925, 0f43600000;
	mul.rn.f32 	%r931, %r208, %r930;
	add.rn.f32 	%r1154, %r929, %r931;
	mul.rn.f32 	%r932, %r209, %r930;
	add.rn.f32 	%r1155, %r929, %r932;
$L__BB0_306:
	st.local.b32 	[%rd1], %r1153;
	st.local.b32 	[%rd1+4], %r1154;
	st.local.b32 	[%rd1+8], %r1155;
	shl.b64 	%rd112, %rd2, 2;
	add.s64 	%rd113, %rd1, %rd112;
	ld.local.b32 	%r933, [%rd113];
	st.local.v2.b32 	[%rd125], {%r933, %r1155};
$L__BB0_307:
	@%p121 bra 	$L__BB0_318;
	cvt.u64.u32 	%rd67, %r221;
	mul.lo.s32 	%r224, %r4, %r1;
	ld.param.b64 	%rd73, [vidcut_place_param_0+8];
	mad.lo.s64 	%rd69, %rd68, %rd67, %rd66;
	cvt.u64.u32 	%rd70, %r224;
	mad.lo.s64 	%rd74, %rd73, %rd67, %rd72;
	add.s64 	%rd4, %rd69, %rd70;
	add.s64 	%rd5, %rd74, %rd70;
	ld.param.b32 	%r1041, [vidcut_place_param_0+148];
	mul.rn.f32 	%r210, %r1067, %r1041;
	ld.param.b32 	%r211, [vidcut_place_param_0+92];
	mov.b32 	%r1156, 0;
	setp.ne.b32 	%p117, %r211, 1;
	ld.param.v2.b32 	{%r212, %r213}, [vidcut_place_param_0+96];
	bra.uni 	$L__BB0_309;
$L__BB0_316:
	st.global.b16 	[%rd62], %r214;
$L__BB0_317:
	cvt.u32.u64 	%r1059, %rd60;
	add.s32 	%r1157, %r1157, -1;
	add.s64 	%rd125, %rd125, 4;
	add.s32 	%r1156, %r1059, %r211;
	setp.eq.b32 	%p120, %r1157, 0;
	@%p120 bra 	$L__BB0_318;
$L__BB0_309:
	cvt.u64.u32 	%rd60, %r1156;
	add.s64 	%rd61, %rd4, %rd60;
	@%p117 bra 	$L__BB0_311;
	ld.global.b8 	%r1158, [%rd61];
	bra.uni 	$L__BB0_312;
$L__BB0_311:
	ld.global.b16 	%r1158, [%rd61];
$L__BB0_312:
	shr.u32 	%r1042, %r1158, %r212;
	and.b32 	%r1043, %r1042, %r213;
	cvt.rn.f32.u32 	%r1159, %r1043;
	@%p20 bra 	$L__BB0_314;
	ld.local.b32 	%r1044, [%rd125];
	sub.rn.f32 	%r1045, %r1044, %r1159;
	mul.rn.f32 	%r1046, %r210, %r1045;
	add.rn.f32 	%r1047, %r1046, %r1159;
	and.b32 	%r1048, %r1047, -2147483648;
	or.b32 	%r1049, %r1048, 1056964608;
	add.rn.f32 	%r1050, %r1047, %r1049;
	cvt.rzi.f32.f32 	%r1051, %r1050;
	abs.f32 	%r1052, %r1047;
	setp.gt.f32 	%p118, %r1052, 0f4B000000;
	selp.f32 	%r1053, %r1047, %r1051, %p118;
	cvt.rzi.f32.f32 	%r1054, %r1047;
	setp.lt.f32 	%p119, %r1052, 0f3F000000;
	selp.f32 	%r1159, %r1054, %r1053, %p119;
$L__BB0_314:
	add.s64 	%rd62, %rd5, %rd60;
	cvt.rn.f32.u32 	%r1055, %r213;
	max.f32 	%r1056, %r1159, 0f00000000;
	min.f32 	%r1057, %r1056, %r1055;
	cvt.rzi.u32.f32 	%r1058, %r1057;
	shl.b32 	%r214, %r1058, %r212;
	@%p117 bra 	$L__BB0_316;
	st.global.b8 	[%rd62], %r214;
	bra.uni 	$L__BB0_317;
$L__BB0_318:
	ret;
}
"#;

const BLOCK: (u32, u32) = (32, 8);

#[repr(C)]
#[derive(Clone, Copy)]
struct Plane {
    out: u64,
    out_pitch: u64,
    under: [u64; 2],
    under_pitch: [u64; 2],
    src: [u64; 2],
    src_pitch: [u64; 2],
    sums: GpuPlane,
}

const _: () = assert!(std::mem::size_of::<Plane>() == 872);

pub(crate) fn place(
    frames: &CudaFrames,
    under: &frame::Video,
    layer: &Layer,
    size: (u32, u32),
) -> Result<Option<frame::Video>> {
    let picture = layer.picture;
    // SAFETY: the pool is a live CUDA frames context.
    let (fc, (ctx, stream)) = unsafe {
        let fc = &*((*frames.as_ptr()).data as *const ffi::AVHWFramesContext);
        (fc, cuda_context(fc.device_ref))
    };
    let sw: format::Pixel = fc.sw_format.into();
    if !matches!(sw, format::Pixel::NV12 | format::Pixel::P010LE)
        || crate::hw::sw_format(picture) != sw
    {
        return Ok(None);
    }
    let Some(area) = layer.area(size) else {
        return crate::export::video::frame_ref(under).map(Some);
    };
    let Some(sums) = layer.gpu_planes(under, frame_u32(fc.width, fc.height)?, size, area)? else {
        return Ok(None);
    };
    let cuda = Cuda::get()?;
    super::share::wait_for(under)?;
    super::share::wait_for(picture)?;
    let mut out = pool_frame(frames)?;
    let function = kernel_of(cuda, ctx, PTX, c"vidcut_place")?;
    let planes = |f: &frame::Video| -> Result<([u64; 2], [u64; 2])> {
        // SAFETY: a CUDA frame's planes are device pointers.
        let f = unsafe { &*f.as_ptr() };
        Ok((
            [f.data[0] as u64, f.data[1] as u64],
            [stride_u64(f.linesize[0])?, stride_u64(f.linesize[1])?],
        ))
    };
    let ((o, o_pitch), (u, u_pitch), (s, s_pitch)) =
        (planes(&out)?, planes(under)?, planes(picture)?);
    in_context(cuda, ctx, || {
        for (p, sums) in sums.into_iter().enumerate() {
            let mut args = Plane {
                out: o[p],
                out_pitch: o_pitch[p],
                under: u,
                under_pitch: u_pitch,
                src: s,
                src_pitch: s_pitch,
                sums,
            };
            let mut params: [*mut c_void; 1] = [(&raw mut args).cast()];
            // SAFETY: the context is current; `params` holds the kernel's one parameter, laid out as its PTX declares.
            check("placing a picture", unsafe {
                (cuda.launch_kernel)(
                    function,
                    sums.width.div_ceil(BLOCK.0),
                    sums.height.div_ceil(BLOCK.1),
                    1,
                    BLOCK.0,
                    BLOCK.1,
                    1,
                    0,
                    stream,
                    params.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            })?;
        }
        Ok(())
    })?;
    // SAFETY: both are valid frames.
    unsafe { ffi::av_frame_copy_props(out.as_mut_ptr(), under.as_ptr()) };
    Ok(Some(out))
}
