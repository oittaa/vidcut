use std::ffi::c_void;

use vidcut_core::render::WHOLE;

use super::CudaFrames;
use super::burn::pool_frame;
use super::mix::{frame_u32, kernel_of, stride_u64};
use super::share::{Cuda, check, cuda_context, in_context};
use crate::error::Result;
use crate::export::mix::Samples;
use crate::export::place::Layer;
use crate::ffmpeg::{ffi, format, frame};

const PTX: &str = r#"
.version 7.0
.target sm_52
.address_size 64
.visible .entry vidcut_place(
	.param .align 8 .b8 vidcut_place_param_0[768]
)
{
	.reg .pred 	%p<40>;
	.reg .b32 	%r<345>;
	.reg .b64 	%rd<38>;
	mov.u32 	%r66, %ctaid.x;
	mov.u32 	%r67, %ntid.x;
	mov.u32 	%r68, %tid.x;
	mad.lo.s32 	%r1, %r66, %r67, %r68;
	mov.u32 	%r69, %ctaid.y;
	mov.u32 	%r70, %ntid.y;
	mov.u32 	%r71, %tid.y;
	mad.lo.s32 	%r72, %r69, %r70, %r71;
	ld.param.v2.b32 	{%r73, %r74}, [vidcut_place_param_0+48];
	setp.ge.u32 	%p1, %r1, %r73;
	setp.ge.u32 	%p2, %r72, %r74;
	or.pred 	%p3, %p1, %p2;
	@%p3 bra 	$L__BB0_64;
	mov.b64 	%rd18, vidcut_place_param_0;
	ld.param.v2.b32 	{%r2, %r3}, [vidcut_place_param_0+56];
	ld.param.b64 	%rd19, [vidcut_place_param_0+16];
	cvta.to.global.u64 	%rd20, %rd19;
	ld.param.b64 	%rd25, [vidcut_place_param_0];
	cvta.to.global.u64 	%rd26, %rd25;
	ld.param.v2.b32 	{%r76, %r77}, [vidcut_place_param_0+72];
	setp.lt.u32 	%p4, %r1, %r76;
	ld.param.v2.b32 	{%r78, %r79}, [vidcut_place_param_0+80];
	setp.ge.u32 	%p5, %r1, %r78;
	setp.lt.u32 	%p6, %r72, %r77;
	or.pred 	%p7, %p4, %p6;
	or.pred 	%p8, %p7, %p5;
	setp.ge.u32 	%p9, %r72, %r79;
	mov.pred 	%p39, 0;
	mov.b32 	%r339, 0f00000000;
	or.pred 	%p10, %p8, %p9;
	mov.b32 	%r324, %r339;
	@%p10 bra 	$L__BB0_11;
	ld.param.v2.b32 	{%r5, %r82}, [vidcut_place_param_0+104];
	setp.eq.b32 	%p11, %r5, 0;
	mov.b32 	%r323, 0f3F800000;
	mov.pred 	%p38, 0;
	@%p11 bra 	$L__BB0_10;
	cvt.rn.f32.u32 	%r80, %r1;
	add.rn.f32 	%r81, %r80, 0f3F000000;
	mul.rn.f32 	%r6, %r81, %r82;
	cvt.rn.f32.u32 	%r83, %r72;
	add.rn.f32 	%r84, %r83, 0f3F000000;
	ld.param.b32 	%r85, [vidcut_place_param_0+112];
	mul.rn.f32 	%r7, %r84, %r85;
	add.s32 	%r321, %r5, -1;
	add.s64 	%rd37, %rd18, 232;
	mov.b32 	%r323, 0f3F800000;
	bra.uni 	$L__BB0_4;
$L__BB0_8:
	mul.rn.f32 	%r103, %r8, %r14;
	sub.rn.f32 	%r104, %r12, %r103;
	mul.rn.f32 	%r105, %r9, %r14;
	sub.rn.f32 	%r106, %r13, %r105;
	mul.rn.f32 	%r107, %r8, %r17;
	sub.rn.f32 	%r108, %r15, %r107;
	mul.rn.f32 	%r109, %r9, %r17;
	sub.rn.f32 	%r110, %r16, %r109;
	mul.rn.f32 	%r111, %r104, %r104;
	mul.rn.f32 	%r112, %r106, %r106;
	add.rn.f32 	%r113, %r111, %r112;
	sqrt.rn.f32 	%r114, %r113;
	div.rn.f32 	%r115, %r114, %r11;
	mul.rn.f32 	%r116, %r108, %r108;
	mul.rn.f32 	%r117, %r110, %r110;
	add.rn.f32 	%r118, %r116, %r117;
	sqrt.rn.f32 	%r119, %r118;
	div.rn.f32 	%r120, %r119, %r11;
	ld.param.b32 	%r121, [%rd37+-40];
	sub.rn.f32 	%r122, %r14, %r121;
	div.rn.f32 	%r123, %r122, %r115;
	add.rn.f32 	%r124, %r123, 0f3F000000;
	max.f32 	%r125, %r124, 0f00000000;
	min.f32 	%r126, %r125, 0f3F800000;
	mul.rn.f32 	%r127, %r323, %r126;
	ld.param.b32 	%r128, [%rd37+-32];
	sub.rn.f32 	%r129, %r128, %r14;
	div.rn.f32 	%r130, %r129, %r115;
	add.rn.f32 	%r131, %r130, 0f3F000000;
	max.f32 	%r132, %r131, 0f00000000;
	min.f32 	%r133, %r132, 0f3F800000;
	mul.rn.f32 	%r134, %r127, %r133;
	ld.param.b32 	%r135, [%rd37+-36];
	sub.rn.f32 	%r136, %r17, %r135;
	div.rn.f32 	%r137, %r136, %r120;
	add.rn.f32 	%r138, %r137, 0f3F000000;
	max.f32 	%r139, %r138, 0f00000000;
	min.f32 	%r140, %r139, 0f3F800000;
	mul.rn.f32 	%r141, %r134, %r140;
	ld.param.b32 	%r142, [%rd37+-28];
	sub.rn.f32 	%r143, %r142, %r17;
	div.rn.f32 	%r144, %r143, %r120;
	add.rn.f32 	%r145, %r144, 0f3F000000;
	max.f32 	%r146, %r145, 0f00000000;
	min.f32 	%r147, %r146, 0f3F800000;
	mul.rn.f32 	%r18, %r141, %r147;
	ld.param.b32 	%r184, [%rd37];
	mul.rn.f32 	%r185, %r322, %r184;
	mul.rn.f32 	%r323, %r18, %r185;
	setp.leu.f32 	%p38, %r323, 0f00000000;
$L__BB0_9:
	setp.ne.b32 	%p17, %r321, 0;
	not.pred 	%p18, %p38;
	add.s32 	%r321, %r321, -1;
	add.s64 	%rd37, %rd37, 76;
	and.pred 	%p19, %p18, %p17;
	@!%p19 bra 	$L__BB0_10;
$L__BB0_4:
	ld.param.b32 	%r8, [%rd37+-48];
	mul.rn.f32 	%r86, %r6, %r8;
	ld.param.b32 	%r9, [%rd37+-44];
	mul.rn.f32 	%r87, %r7, %r9;
	add.rn.f32 	%r88, %r86, %r87;
	add.rn.f32 	%r10, %r88, 0f3F800000;
	abs.f32 	%r11, %r10;
	shl.b32 	%r89, %r10, 1;
	setp.gt.u32 	%p12, %r89, -16777217;
	setp.lt.f32 	%p13, %r11, 0f2B8CBCCC;
	or.pred 	%p14, %p12, %p13;
	mov.pred 	%p38, -1;
	@%p14 bra 	$L__BB0_9;
	ld.param.b32 	%r12, [%rd37+-72];
	mul.rn.f32 	%r90, %r6, %r12;
	ld.param.b32 	%r13, [%rd37+-68];
	mul.rn.f32 	%r91, %r7, %r13;
	add.rn.f32 	%r92, %r90, %r91;
	ld.param.b32 	%r93, [%rd37+-64];
	add.rn.f32 	%r94, %r93, %r92;
	div.rn.f32 	%r14, %r94, %r10;
	ld.param.b32 	%r15, [%rd37+-60];
	mul.rn.f32 	%r95, %r6, %r15;
	ld.param.b32 	%r16, [%rd37+-56];
	mul.rn.f32 	%r96, %r7, %r16;
	add.rn.f32 	%r97, %r95, %r96;
	ld.param.b32 	%r98, [%rd37+-52];
	add.rn.f32 	%r99, %r98, %r97;
	div.rn.f32 	%r17, %r99, %r10;
	shl.b32 	%r100, %r14, 1;
	shl.b32 	%r101, %r17, 1;
	max.u32 	%r102, %r100, %r101;
	setp.gt.u32 	%p15, %r102, -16777217;
	@%p15 bra 	$L__BB0_9;
	ld.param.b32 	%r19, [%rd37+-8];
	ld.param.b32 	%r148, [%rd37+-4];
	min.f32 	%r149, %r19, %r148;
	setp.le.f32 	%p16, %r149, 0f00000000;
	mov.b32 	%r322, 0f3F800000;
	@%p16 bra 	$L__BB0_8;
	ld.param.b32 	%r150, [%rd37+-24];
	sub.rn.f32 	%r151, %r150, %r14;
	ld.param.b32 	%r152, [%rd37+-16];
	sub.rn.f32 	%r153, %r14, %r152;
	max.f32 	%r154, %r151, %r153;
	max.f32 	%r155, %r154, 0f00000000;
	div.rn.f32 	%r156, %r155, %r19;
	ld.param.b32 	%r157, [%rd37+-20];
	sub.rn.f32 	%r158, %r157, %r17;
	ld.param.b32 	%r159, [%rd37+-12];
	sub.rn.f32 	%r160, %r17, %r159;
	max.f32 	%r161, %r158, %r160;
	max.f32 	%r162, %r161, 0f00000000;
	div.rn.f32 	%r163, %r162, %r148;
	sub.rn.f32 	%r164, %r156, %r163;
	abs.f32 	%r165, %r164;
	mov.b32 	%r166, 0f3F000000;
	sub.rn.f32 	%r167, %r166, %r165;
	max.f32 	%r168, %r167, 0f00000000;
	add.rn.f32 	%r169, %r168, %r168;
	max.f32 	%r170, %r156, %r163;
	mul.rn.f32 	%r171, %r169, %r169;
	mul.rn.f32 	%r172, %r171, 0f3F000000;
	mul.rn.f32 	%r173, %r172, 0f3E800000;
	add.rn.f32 	%r174, %r170, %r173;
	mov.b32 	%r175, 0f3F800000;
	sub.rn.f32 	%r176, %r175, %r174;
	div.rn.f32 	%r177, %r176, 0f3F600000;
	max.f32 	%r178, %r177, 0f00000000;
	min.f32 	%r179, %r178, 0f3F800000;
	mul.rn.f32 	%r180, %r179, %r179;
	add.rn.f32 	%r181, %r179, %r179;
	mov.b32 	%r182, 0f40400000;
	sub.rn.f32 	%r183, %r182, %r181;
	mul.rn.f32 	%r322, %r180, %r183;
	bra.uni 	$L__BB0_8;
$L__BB0_10:
	selp.f32 	%r324, 0f00000000, %r323, %p38;
	not.pred 	%p39, %p38;
$L__BB0_11:
	mul.lo.s32 	%r4, %r3, %r2;
	not.pred 	%p20, %p39;
	setp.eq.b32 	%p36, %r2, 0;
	setp.ne.b32 	%p37, %r3, 1;
	mov.b32 	%r340, %r339;
	@%p20 bra 	$L__BB0_48;
	ld.param.b64 	%rd30, [vidcut_place_param_0+32];
	cvta.to.global.u64 	%rd4, %rd30;
	ld.param.v2.b32 	{%r20, %r21}, [vidcut_place_param_0+96];
	setp.eq.b32 	%p21, %r21, 0;
	mov.b32 	%r337, 0f00000000;
	mov.b32 	%r338, %r337;
	@%p21 bra 	$L__BB0_47;
	ld.param.b32 	%r186, [vidcut_place_param_0+104];
	add.s32 	%r187, %r186, -1;
	mul.wide.u32 	%rd29, %r187, 76;
	add.s64 	%rd3, %rd18, %rd29;
	cvt.rn.f32.u32 	%r188, %r1;
	cvt.rn.f32.u32 	%r22, %r20;
	cvt.rn.f32.u32 	%r189, %r72;
	cvt.rn.f32.u32 	%r23, %r21;
	ld.param.b32 	%r24, [vidcut_place_param_0+108];
	mul.rn.f32 	%r25, %r24, %r188;
	ld.param.v2.b32 	{%r26, %r27}, [vidcut_place_param_0+112];
	mul.rn.f32 	%r28, %r26, %r189;
	ld.param.v2.b32 	{%r29, %r30}, [vidcut_place_param_0+120];
	ld.param.v2.b32 	{%r31, %r32}, [vidcut_place_param_0+128];
	ld.param.v2.b32 	{%r33, %r34}, [vidcut_place_param_0+136];
	ld.param.v2.b32 	{%r35, %r190}, [vidcut_place_param_0+144];
	max.f32 	%r36, %r190, %r34;
	ld.param.b32 	%r191, [vidcut_place_param_0+152];
	max.f32 	%r37, %r191, %r35;
	ld.param.b64 	%rd5, [vidcut_place_param_0+40];
	ld.param.v2.b32 	{%r192, %r193}, [vidcut_place_param_0+88];
	add.s32 	%r38, %r193, -1;
	add.s32 	%r39, %r192, -1;
	ld.param.v2.b32 	{%r40, %r41}, [vidcut_place_param_0+64];
	add.s32 	%r42, %r2, -1;
	mov.b32 	%r325, 0;
	mov.b32 	%r338, 0f00000000;
	setp.eq.b32 	%p22, %r20, 0;
	setp.eq.b32 	%p26, %r2, 1;
	mov.b32 	%r337, %r338;
	bra.uni 	$L__BB0_14;
$L__BB0_46:
	add.s32 	%r325, %r325, 1;
	setp.eq.b32 	%p29, %r325, %r21;
	@%p29 bra 	$L__BB0_47;
$L__BB0_14:
	@%p22 bra 	$L__BB0_46;
	cvt.rn.f32.u32 	%r194, %r325;
	add.rn.f32 	%r195, %r194, 0f3F000000;
	div.rn.f32 	%r196, %r195, %r23;
	mul.rn.f32 	%r197, %r196, %r26;
	add.rn.f32 	%r43, %r28, %r197;
	ld.param.b32 	%r44, [%rd3+184];
	ld.param.b32 	%r198, [%rd3+188];
	mul.rn.f32 	%r45, %r198, %r43;
	mov.b32 	%r326, 0;
	bra.uni 	$L__BB0_16;
$L__BB0_45:
	add.s32 	%r326, %r326, 1;
	setp.eq.b32 	%p28, %r326, %r20;
	@%p28 bra 	$L__BB0_46;
$L__BB0_16:
	cvt.rn.f32.u32 	%r199, %r326;
	add.rn.f32 	%r200, %r199, 0f3F000000;
	div.rn.f32 	%r201, %r200, %r22;
	mul.rn.f32 	%r202, %r24, %r201;
	add.rn.f32 	%r46, %r25, %r202;
	mul.rn.f32 	%r203, %r44, %r46;
	add.rn.f32 	%r204, %r45, %r203;
	add.rn.f32 	%r47, %r204, 0f3F800000;
	abs.f32 	%r205, %r47;
	shl.b32 	%r206, %r47, 1;
	setp.gt.u32 	%p23, %r206, -16777217;
	setp.lt.f32 	%p24, %r205, 0f2B8CBCCC;
	or.pred 	%p25, %p23, %p24;
	@%p25 bra 	$L__BB0_45;
	@%p36 bra 	$L__BB0_45;
	ld.param.b32 	%r207, [%rd3+160];
	mul.rn.f32 	%r208, %r46, %r207;
	ld.param.b32 	%r209, [%rd3+164];
	mul.rn.f32 	%r210, %r43, %r209;
	add.rn.f32 	%r211, %r208, %r210;
	ld.param.b32 	%r212, [%rd3+168];
	add.rn.f32 	%r213, %r212, %r211;
	div.rn.f32 	%r214, %r213, %r47;
	ld.param.b32 	%r215, [%rd3+172];
	mul.rn.f32 	%r216, %r46, %r215;
	ld.param.b32 	%r217, [%rd3+176];
	mul.rn.f32 	%r218, %r43, %r217;
	add.rn.f32 	%r219, %r216, %r218;
	ld.param.b32 	%r220, [%rd3+180];
	add.rn.f32 	%r221, %r220, %r219;
	div.rn.f32 	%r222, %r221, %r47;
	mul.rn.f32 	%r223, %r214, %r32;
	add.rn.f32 	%r224, %r30, %r223;
	div.rn.f32 	%r225, %r224, %r27;
	add.rn.f32 	%r226, %r225, 0fBF000000;
	max.f32 	%r227, %r226, %r34;
	min.f32 	%r228, %r227, %r36;
	mul.rn.f32 	%r229, %r222, %r33;
	add.rn.f32 	%r230, %r31, %r229;
	div.rn.f32 	%r231, %r230, %r29;
	add.rn.f32 	%r232, %r231, 0fBF000000;
	max.f32 	%r233, %r232, %r35;
	min.f32 	%r234, %r233, %r37;
	cvt.rmi.f32.f32 	%r48, %r228;
	cvt.rmi.f32.f32 	%r49, %r234;
	sub.rn.f32 	%r50, %r228, %r48;
	cvt.rzi.u32.f32 	%r235, %r49;
	add.s32 	%r236, %r235, 1;
	min.u32 	%r237, %r236, %r38;
	cvt.u64.u32 	%rd33, %r235;
	mad.lo.s64 	%rd34, %rd5, %rd33, %rd4;
	cvt.rzi.u32.f32 	%r238, %r48;
	add.s32 	%r239, %r238, 1;
	min.u32 	%r240, %r239, %r39;
	mul.lo.s32 	%r241, %r4, %r238;
	cvt.u64.u32 	%rd35, %r241;
	add.s64 	%rd6, %rd34, %rd35;
	mul.lo.s32 	%r243, %r240, %r4;
	cvt.u64.u32 	%rd36, %r243;
	@%p37 bra 	$L__BB0_19;
	ld.global.b8 	%r327, [%rd6];
	bra.uni 	$L__BB0_21;
$L__BB0_19:
	ld.global.b16 	%r327, [%rd6];
$L__BB0_21:
	cvt.u64.u32 	%rd31, %r237;
	mov.b32 	%r242, 0f3F800000;
	sub.rn.f32 	%r52, %r242, %r50;
	add.s64 	%rd7, %rd34, %rd36;
	shr.u32 	%r244, %r327, %r40;
	and.b32 	%r245, %r244, %r41;
	cvt.rn.f32.u32 	%r246, %r245;
	@%p37 bra 	$L__BB0_22;
	ld.global.b8 	%r328, [%rd7];
	bra.uni 	$L__BB0_24;
$L__BB0_22:
	ld.global.b16 	%r328, [%rd7];
$L__BB0_24:
	sub.rn.f32 	%r51, %r234, %r49;
	mad.lo.s64 	%rd32, %rd5, %rd31, %rd4;
	mul.rn.f32 	%r54, %r52, %r246;
	add.s64 	%rd8, %rd32, %rd35;
	shr.u32 	%r247, %r328, %r40;
	and.b32 	%r248, %r247, %r41;
	cvt.rn.f32.u32 	%r249, %r248;
	mul.rn.f32 	%r250, %r50, %r249;
	@%p37 bra 	$L__BB0_25;
	ld.global.b8 	%r329, [%rd8];
	bra.uni 	$L__BB0_27;
$L__BB0_25:
	ld.global.b16 	%r329, [%rd8];
$L__BB0_27:
	add.s64 	%rd9, %rd32, %rd36;
	sub.rn.f32 	%r53, %r242, %r51;
	add.rn.f32 	%r55, %r54, %r250;
	shr.u32 	%r251, %r329, %r40;
	and.b32 	%r252, %r251, %r41;
	cvt.rn.f32.u32 	%r253, %r252;
	mul.rn.f32 	%r56, %r52, %r253;
	@%p37 bra 	$L__BB0_28;
	ld.global.b8 	%r330, [%rd9];
	bra.uni 	$L__BB0_30;
$L__BB0_28:
	ld.global.b16 	%r330, [%rd9];
$L__BB0_30:
	shr.u32 	%r254, %r330, %r40;
	and.b32 	%r255, %r254, %r41;
	cvt.rn.f32.u32 	%r256, %r255;
	mul.rn.f32 	%r257, %r50, %r256;
	add.rn.f32 	%r258, %r56, %r257;
	mul.rn.f32 	%r259, %r53, %r55;
	mul.rn.f32 	%r260, %r51, %r258;
	add.rn.f32 	%r261, %r259, %r260;
	add.rn.f32 	%r338, %r338, %r261;
	@%p26 bra 	$L__BB0_45;
	mov.b32 	%r331, %r3;
	mov.b32 	%r332, %r42;
	bra.uni 	$L__BB0_32;
$L__BB0_43:
	ld.global.b16 	%r336, [%rd14];
$L__BB0_44:
	cvt.u32.u64 	%r272, %rd10;
	shr.u32 	%r273, %r336, %r40;
	and.b32 	%r274, %r273, %r41;
	cvt.rn.f32.u32 	%r275, %r274;
	mul.rn.f32 	%r276, %r50, %r275;
	add.rn.f32 	%r277, %r59, %r276;
	mul.rn.f32 	%r278, %r53, %r58;
	mul.rn.f32 	%r279, %r51, %r277;
	add.rn.f32 	%r280, %r278, %r279;
	add.rn.f32 	%r337, %r337, %r280;
	add.s32 	%r332, %r332, -1;
	add.s32 	%r331, %r272, %r3;
	setp.ne.b32 	%p27, %r332, 0;
	@!%p27 bra 	$L__BB0_45;
$L__BB0_32:
	cvt.u64.u32 	%rd10, %r331;
	add.s64 	%rd11, %rd6, %rd10;
	@%p37 bra 	$L__BB0_34;
	ld.global.b8 	%r333, [%rd11];
	bra.uni 	$L__BB0_35;
$L__BB0_34:
	ld.global.b16 	%r333, [%rd11];
$L__BB0_35:
	shr.u32 	%r262, %r333, %r40;
	and.b32 	%r263, %r262, %r41;
	cvt.rn.f32.u32 	%r264, %r263;
	add.s64 	%rd12, %rd7, %rd10;
	@%p37 bra 	$L__BB0_37;
	ld.global.b8 	%r334, [%rd12];
	bra.uni 	$L__BB0_38;
$L__BB0_37:
	ld.global.b16 	%r334, [%rd12];
$L__BB0_38:
	mul.rn.f32 	%r57, %r52, %r264;
	shr.u32 	%r265, %r334, %r40;
	and.b32 	%r266, %r265, %r41;
	cvt.rn.f32.u32 	%r267, %r266;
	mul.rn.f32 	%r268, %r50, %r267;
	add.s64 	%rd13, %rd8, %rd10;
	@%p37 bra 	$L__BB0_40;
	ld.global.b8 	%r335, [%rd13];
	bra.uni 	$L__BB0_41;
$L__BB0_40:
	ld.global.b16 	%r335, [%rd13];
$L__BB0_41:
	add.rn.f32 	%r58, %r57, %r268;
	shr.u32 	%r269, %r335, %r40;
	and.b32 	%r270, %r269, %r41;
	cvt.rn.f32.u32 	%r271, %r270;
	mul.rn.f32 	%r59, %r52, %r271;
	add.s64 	%rd14, %rd9, %rd10;
	@%p37 bra 	$L__BB0_43;
	ld.global.b8 	%r336, [%rd14];
	bra.uni 	$L__BB0_44;
$L__BB0_47:
	mul.lo.s32 	%r281, %r20, %r21;
	cvt.rn.f32.u32 	%r282, %r281;
	div.rn.f32 	%r339, %r338, %r282;
	div.rn.f32 	%r340, %r337, %r282;
$L__BB0_48:
	@%p36 bra 	$L__BB0_64;
	cvt.u64.u32 	%rd21, %r72;
	ld.param.b64 	%rd22, [vidcut_place_param_0+24];
	mul.lo.s32 	%r75, %r4, %r1;
	ld.param.b64 	%rd27, [vidcut_place_param_0+8];
	mad.lo.s64 	%rd23, %rd22, %rd21, %rd20;
	cvt.u64.u32 	%rd24, %r75;
	mad.lo.s64 	%rd28, %rd27, %rd21, %rd26;
	add.s64 	%rd1, %rd23, %rd24;
	ld.param.b32 	%r283, [vidcut_place_param_0+156];
	mul.rn.f32 	%r60, %r324, %r283;
	@%p37 bra 	$L__BB0_50;
	ld.global.b8 	%r341, [%rd1];
	bra.uni 	$L__BB0_52;
$L__BB0_50:
	ld.global.b16 	%r341, [%rd1];
$L__BB0_52:
	add.s64 	%rd2, %rd28, %rd24;
	ld.param.v2.b32 	{%r61, %r62}, [vidcut_place_param_0+64];
	shr.u32 	%r284, %r341, %r61;
	and.b32 	%r285, %r284, %r62;
	cvt.rn.f32.u32 	%r286, %r285;
	sub.rn.f32 	%r287, %r339, %r286;
	mul.rn.f32 	%r288, %r60, %r287;
	selp.f32 	%r289, %r288, 0f80000000, %p39;
	add.rn.f32 	%r290, %r289, %r286;
	and.b32 	%r291, %r290, -2147483648;
	or.b32 	%r292, %r291, 1056964608;
	add.rn.f32 	%r293, %r290, %r292;
	cvt.rzi.f32.f32 	%r294, %r293;
	abs.f32 	%r295, %r290;
	setp.gt.f32 	%p30, %r295, 0f4B000000;
	selp.f32 	%r296, %r290, %r294, %p30;
	cvt.rzi.f32.f32 	%r297, %r290;
	setp.lt.f32 	%p31, %r295, 0f3F000000;
	selp.f32 	%r298, %r297, %r296, %p31;
	cvt.rn.f32.u32 	%r63, %r62;
	max.f32 	%r299, %r298, 0f00000000;
	min.f32 	%r300, %r299, %r63;
	cvt.rzi.u32.f32 	%r301, %r300;
	shl.b32 	%r64, %r301, %r61;
	@%p37 bra 	$L__BB0_53;
	st.global.b8 	[%rd2], %r64;
	bra.uni 	$L__BB0_55;
$L__BB0_53:
	st.global.b16 	[%rd2], %r64;
$L__BB0_55:
	setp.lt.u32 	%p32, %r2, 2;
	@%p32 bra 	$L__BB0_64;
	mov.b32 	%r343, 1;
	mov.b32 	%r342, %r3;
	bra.uni 	$L__BB0_57;
$L__BB0_62:
	st.global.b16 	[%rd17], %r65;
$L__BB0_63:
	cvt.u32.u64 	%r320, %rd15;
	add.s32 	%r343, %r343, 1;
	add.s32 	%r342, %r320, %r3;
	setp.lt.u32 	%p35, %r343, %r2;
	@!%p35 bra 	$L__BB0_64;
$L__BB0_57:
	cvt.u64.u32 	%rd15, %r342;
	add.s64 	%rd16, %rd1, %rd15;
	@%p37 bra 	$L__BB0_59;
	ld.global.b8 	%r344, [%rd16];
	bra.uni 	$L__BB0_60;
$L__BB0_59:
	ld.global.b16 	%r344, [%rd16];
$L__BB0_60:
	shr.u32 	%r302, %r344, %r61;
	and.b32 	%r303, %r302, %r62;
	cvt.rn.f32.u32 	%r304, %r303;
	sub.rn.f32 	%r305, %r340, %r304;
	mul.rn.f32 	%r306, %r60, %r305;
	selp.f32 	%r307, %r306, 0f80000000, %p39;
	add.rn.f32 	%r308, %r307, %r304;
	add.s64 	%rd17, %rd2, %rd15;
	and.b32 	%r309, %r308, -2147483648;
	or.b32 	%r310, %r309, 1056964608;
	add.rn.f32 	%r311, %r308, %r310;
	cvt.rzi.f32.f32 	%r312, %r311;
	abs.f32 	%r313, %r308;
	setp.gt.f32 	%p33, %r313, 0f4B000000;
	selp.f32 	%r314, %r308, %r312, %p33;
	cvt.rzi.f32.f32 	%r315, %r308;
	setp.lt.f32 	%p34, %r313, 0f3F000000;
	selp.f32 	%r316, %r315, %r314, %p34;
	max.f32 	%r317, %r316, 0f00000000;
	min.f32 	%r318, %r317, %r63;
	cvt.rzi.u32.f32 	%r319, %r318;
	shl.b32 	%r65, %r319, %r61;
	@%p37 bra 	$L__BB0_62;
	st.global.b8 	[%rd17], %r65;
	bra.uni 	$L__BB0_63;
$L__BB0_64:
	ret;
}
"#;

const LEVELS: usize = 8;
const BLOCK: (u32, u32) = (32, 8);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Level {
    back: [f32; 8],
    edge: [f32; 4],
    inner: [f32; 4],
    reach: [f32; 2],
    opacity: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Plane {
    out: u64,
    out_pitch: u64,
    under: u64,
    under_pitch: u64,
    src: u64,
    src_pitch: u64,
    width: u32,
    height: u32,
    comps: u32,
    bytes: u32,
    shift: u32,
    top: u32,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    cols: u32,
    rows: u32,
    tx: u32,
    ty: u32,
    count: u32,
    across: f32,
    down: f32,
    sa: f32,
    sd: f32,
    fitted: [f32; 4],
    lo: [f32; 2],
    hi: [f32; 2],
    level: f32,
    levels: [Level; LEVELS],
}

const _: () = assert!(std::mem::size_of::<Plane>() == 768);

fn levels(layer: &Layer) -> [Level; LEVELS] {
    let mut levels = [Level::default(); LEVELS];
    for (to, l) in levels.iter_mut().zip(&layer.placing.levels) {
        *to = Level {
            back: l.back.map(|v| v as f32),
            edge: l.edge.map(|v| v as f32),
            inner: l.soft.inner.map(|v| v as f32),
            reach: l.soft.reach.map(|v| v as f32),
            opacity: l.opacity as f32,
        };
    }
    levels
}

pub(crate) fn place(
    frames: &CudaFrames,
    under: &frame::Video,
    layer: &Layer,
    size: (u32, u32),
) -> Result<Option<frame::Video>> {
    let (picture, placing) = (layer.picture, layer.placing);
    if placing.levels.is_empty() || placing.levels.len() > LEVELS {
        return Ok(None);
    }
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
    let cuda = Cuda::get()?;
    super::share::wait_for(under)?;
    super::share::wait_for(picture)?;
    let samples = Samples::of(sw)?;
    let (w, h) = frame_u32(fc.width, fc.height)?;
    let mut out = pool_frame(frames)?;
    let function = kernel_of(cuda, ctx, PTX, c"vidcut_place")?;
    let [fx, fy, fw, fh] = layer.fitted;
    let [dux, duy, dvx, dvy] = placing.step();
    let taps = |a: f64, b: f64| (a.hypot(b).ceil() as u32).clamp(1, 8);
    let levels = levels(layer);
    in_context(cuda, ctx, || {
        for (p, g) in [(0usize, 0u32), (1, 1)] {
            let plane = |f: &frame::Video| -> Result<(u64, u64)> {
                // SAFETY: a CUDA frame's planes are device pointers.
                let f = unsafe { &*f.as_ptr() };
                Ok((f.data[p] as u64, stride_u64(f.linesize[p])?))
            };
            let ((o, o_pitch), (u, u_pitch)) = (plane(&out)?, plane(under)?);
            let (s, s_pitch) = plane(picture)?;
            let step = f64::from(1u32 << g);
            let (cols, rows) = (
                picture.width().div_ceil(1 << g),
                picture.height().div_ceil(1 << g),
            );
            let mut args = Plane {
                out: o,
                out_pitch: o_pitch,
                under: u,
                under_pitch: u_pitch,
                src: s,
                src_pitch: s_pitch,
                width: w.div_ceil(1 << g),
                height: h.div_ceil(1 << g),
                comps: if p == 0 { 1 } else { 2 },
                bytes: samples.bytes as u32,
                shift: samples.shift,
                top: samples.top,
                x0: area[0] >> g,
                y0: area[1] >> g,
                x1: area[2].min(size.0).div_ceil(1 << g),
                y1: area[3].min(size.1).div_ceil(1 << g),
                cols,
                rows,
                tx: taps(dux * fw, dvx * fh),
                ty: taps(duy * fw, dvy * fh),
                count: placing.levels.len() as u32,
                across: step as f32,
                down: step as f32,
                sa: step as f32,
                sd: step as f32,
                fitted: layer.fitted.map(|v| v as f32),
                lo: [(fx / step) as f32, (fy / step) as f32],
                hi: [
                    ((fx + fw) / step - 1.0).min(f64::from(cols) - 1.0) as f32,
                    ((fy + fh) / step - 1.0).min(f64::from(rows) - 1.0) as f32,
                ],
                level: (f64::from(layer.weight) / f64::from(WHOLE)) as f32,
                levels,
            };
            let mut params: [*mut c_void; 1] = [(&raw mut args).cast()];
            // SAFETY: the context is current; `params` holds the kernel's one parameter, laid out as its PTX declares.
            check("placing a picture", unsafe {
                (cuda.launch_kernel)(
                    function,
                    args.width.div_ceil(BLOCK.0),
                    args.height.div_ceil(BLOCK.1),
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
