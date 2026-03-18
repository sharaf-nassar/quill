import type { RangeType, UsageBucket } from "../../types";

interface ChartsTabProps {
	range: RangeType;
	onRangeChange: (r: RangeType) => void;
	currentBuckets: UsageBucket[];
}

function ChartsTab(_props: ChartsTabProps) {
	return <div>Charts coming soon</div>;
}

export default ChartsTab;
